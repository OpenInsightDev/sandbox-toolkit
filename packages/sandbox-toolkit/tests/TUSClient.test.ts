import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";

import { Effect, Layer, Option, Stream } from "effect";
import { FetchHttpClient } from "effect/unstable/http";
import { expect, test } from "vite-plus/test";

import * as TUSClient from "../src/TUSClient.ts";

/**
 * A minimal tus 1.0.0 server, enough for the creation, creation-with-upload,
 * core, and termination paths the service drives. It keeps accepted bytes in
 * memory so a test can assert what the wrapper actually transferred.
 */

interface Stored {
  readonly length: number;
  readonly chunks: Array<Buffer>;
  offset: number;
  terminated: boolean;
}

interface TusStub {
  baseUrl: string;
  readonly uploads: Map<string, Stored>;
  holdPatch: boolean;
  failCreate: boolean;
  stop: () => Promise<void>;
}

const readBody = async (request: IncomingMessage): Promise<Buffer> => {
  const chunks: Array<Buffer> = [];

  for await (const chunk of request) {
    chunks.push(chunk as Buffer);
  }

  return Buffer.concat(chunks);
};

const startTusStub = async (): Promise<TusStub> => {
  const uploads = new Map<string, Stored>();
  const releases: Array<() => void> = [];

  const stub: TusStub = {
    baseUrl: "",
    uploads,
    holdPatch: false,
    failCreate: false,
    stop: async () => {},
  };

  let counter = 0;

  const handle = async (request: IncomingMessage, response: ServerResponse) => {
    const url = new URL(request.url ?? "/", stub.baseUrl);
    const path = url.pathname;
    const id = path.startsWith("/upload/") ? path.slice("/upload/".length) : undefined;

    if (request.method === "POST" && path === "/upload") {
      const body = await readBody(request);

      if (stub.failCreate) {
        response.writeHead(500, { "Tus-Resumable": "1.0.0" });
        response.end();

        return;
      }

      const uploadId = `u${++counter}`;
      const length = Number(request.headers["upload-length"] ?? body.length);

      uploads.set(uploadId, {
        length,
        chunks: body.length === 0 ? [] : [body],
        offset: body.length,
        terminated: false,
      });

      response.writeHead(201, {
        "Tus-Resumable": "1.0.0",
        Location: `${stub.baseUrl}/upload/${uploadId}`,
        "Upload-Offset": String(body.length),
      });
      response.end();

      return;
    }

    const upload = id === undefined ? undefined : uploads.get(id);

    if (upload === undefined) {
      response.writeHead(404, { "Tus-Resumable": "1.0.0" });
      response.end();

      return;
    }

    if (request.method === "HEAD") {
      response.writeHead(200, {
        "Tus-Resumable": "1.0.0",
        "Upload-Offset": String(upload.offset),
        "Upload-Length": String(upload.length),
      });
      response.end();

      return;
    }

    if (request.method === "PATCH") {
      const body = await readBody(request);

      if (stub.holdPatch) {
        await new Promise<void>((resolve) => releases.push(resolve));
      }

      // A destroyed response means the client aborted the transfer while we held
      // the request; there is no one left to answer.
      if (response.destroyed) return;

      upload.chunks.push(body);
      upload.offset += body.length;

      response.writeHead(204, {
        "Tus-Resumable": "1.0.0",
        "Upload-Offset": String(upload.offset),
      });
      response.end();

      return;
    }

    if (request.method === "DELETE") {
      upload.terminated = true;
      if (id !== undefined) uploads.delete(id);

      response.writeHead(204, { "Tus-Resumable": "1.0.0" });
      response.end();

      return;
    }

    response.writeHead(405, { "Tus-Resumable": "1.0.0" });
    response.end();
  };

  const server: Server = createServer((request, response) => {
    void handle(request, response);
  });

  await new Promise<void>((done) => server.listen(0, "127.0.0.1", done));

  const address = server.address();

  if (address === null || typeof address === "string") {
    throw new Error("the tus stub did not bind a port");
  }

  stub.baseUrl = `http://127.0.0.1:${address.port}`;
  stub.stop = () =>
    new Promise<void>((done) => {
      while (releases.length > 0) releases.shift()?.();
      server.closeAllConnections();
      server.close(() => done());
    });

  return stub;
};

const layerOf = (baseUrl: string) =>
  TUSClient.layer({ baseUrl }).pipe(Layer.provide(FetchHttpClient.layer));

const run = <A, E>(
  program: Effect.Effect<A, E, TUSClient.TUSClient>,
  baseUrl: string,
): Promise<A> => Effect.runPromise(Effect.provide(program, layerOf(baseUrl)));

const uploadResult = (options: Parameters<TUSClient.TUSClient["uploadResult"]>[0]) =>
  Effect.flatMap(TUSClient.TUSClient, (client) => client.uploadResult(options));

const content = Buffer.from("hello tus world, from the sandbox toolkit");

test("streams progress and finishes with a result", async () => {
  const stub = await startTusStub();

  try {
    const events = await run(
      Effect.flatMap(TUSClient.TUSClient, (client) =>
        Stream.runCollect(client.upload({ source: content, chunkSize: 8 })),
      ),
      stub.baseUrl,
    );

    const list = Array.from(events);

    expect(list.some(TUSClient.UploadEvent.$is("Progress"))).toBe(true);
    expect(list.filter(TUSClient.UploadEvent.$is("UrlAvailable"))).toHaveLength(1);
    expect(list.filter(TUSClient.UploadEvent.$is("Success"))).toHaveLength(1);

    const success = list.find(TUSClient.UploadEvent.$is("Success"));

    if (success === undefined) throw new Error("missing Success event");

    expect(success.result.status).toBe(204);
    expect(success.result.url).toBe(`${stub.baseUrl}/upload/u1`);

    const stored = stub.uploads.get("u1");

    expect(stored?.offset).toBe(content.length);
    expect(Buffer.concat(stored?.chunks ?? []).equals(content)).toBe(true);
  } finally {
    await stub.stop();
  }
});

test("collects the result without progress notifications", async () => {
  const stub = await startTusStub();

  try {
    const result = await run(uploadResult({ source: content }), stub.baseUrl);

    expect(result.url).toBe(`${stub.baseUrl}/upload/u1`);
    expect(result.status).toBe(204);
    expect(Buffer.concat(stub.uploads.get("u1")?.chunks ?? []).equals(content)).toBe(true);
  } finally {
    await stub.stop();
  }
});

test("uploads in one creation request when creation-with-upload is enabled", async () => {
  const stub = await startTusStub();

  try {
    const result = await run(
      uploadResult({ source: content, uploadDataDuringCreation: true }),
      stub.baseUrl,
    );

    expect(result.status).toBe(201);
    expect(stub.uploads.get("u1")?.offset).toBe(content.length);
  } finally {
    await stub.stop();
  }
});

test("resumes from a partially uploaded resource", async () => {
  const stub = await startTusStub();

  try {
    const split = 10;

    stub.uploads.set("seed", {
      length: content.length,
      chunks: [content.subarray(0, split)],
      offset: split,
      terminated: false,
    });

    const result = await run(
      uploadResult({
        source: content,
        uploadUrl: `${stub.baseUrl}/upload/seed`,
        chunkSize: 4,
      }),
      stub.baseUrl,
    );

    const stored = stub.uploads.get("seed");

    expect(result.url).toBe(`${stub.baseUrl}/upload/seed`);
    expect(stored?.offset).toBe(content.length);
    expect(Buffer.concat(stored?.chunks ?? []).equals(content)).toBe(true);
  } finally {
    await stub.stop();
  }
});

test("interrupting an upload aborts the transfer but keeps the resource", async () => {
  const stub = await startTusStub();

  try {
    stub.holdPatch = true;

    const outcome = await run(
      Effect.timeoutOption(
        uploadResult({ source: content, chunkSize: 4, retryDelays: [] }),
        "300 millis",
      ),
      stub.baseUrl,
    );

    expect(Option.isNone(outcome)).toBe(true);

    const stored = stub.uploads.get("u1");

    expect(stored?.offset).toBe(0);
    expect(stored?.terminated).toBe(false);
  } finally {
    await stub.stop();
  }
});

test("terminates a finished upload resource", async () => {
  const stub = await startTusStub();

  try {
    const result = await run(uploadResult({ source: content }), stub.baseUrl);

    await run(
      Effect.flatMap(TUSClient.TUSClient, (client) => client.terminate(result.url)),
      stub.baseUrl,
    );

    expect(stub.uploads.has("u1")).toBe(false);
  } finally {
    await stub.stop();
  }
});

test("maps a failed creation onto UploadError with the response status", async () => {
  const stub = await startTusStub();

  try {
    stub.failCreate = true;

    const error = await run(
      Effect.flip(uploadResult({ source: content, retryDelays: [] })),
      stub.baseUrl,
    );

    expect(error).toBeInstanceOf(TUSClient.UploadError);
    expect(error.status).toBe(500);
    expect(error.uploadUrl).toBeUndefined();
  } finally {
    await stub.stop();
  }
});
