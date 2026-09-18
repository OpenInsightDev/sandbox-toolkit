import { Effect } from "effect";
import { FetchHttpClient } from "effect/unstable/http";
import { describe, expect, it } from "vite-plus/test";

import { SandboxToolkit } from "../src/client.ts";
import type { SandboxToolkitError } from "../src/errors.ts";

const BASE_URL = "http://sandbox.test";

interface Call {
  readonly method: string;
  readonly url: string;
  readonly accept: string | null;
  readonly contentType: string | null;
  readonly body: string | null;
}

interface Stub {
  readonly calls: Array<Call>;
  readonly fetch: typeof globalThis.fetch;
}

/**
 * Records calls. Provided via `Effect.provideService` because Effect resolves the
 * `Fetch` reference default once per runtime.
 */
const stubFetch = (handler: (call: Call) => Response | Promise<Response>): Stub => {
  const calls: Array<Call> = [];

  const fetchImpl: typeof globalThis.fetch = async (input, init) => {
    const url =
      typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
    const headers = new Headers(init?.headers);
    const call: Call = {
      method: init?.method ?? "GET",
      url,
      accept: headers.get("accept"),
      contentType: headers.get("content-type"),
      body: await readBody(init?.body),
    };
    calls.push(call);
    return handler(call);
  };

  return { calls, fetch: fetchImpl };
};

/// Effect encodes JSON request bodies as bytes.
const readBody = async (body: unknown): Promise<string | null> => {
  if (body === null || body === undefined) return null;
  if (typeof body === "string") return body;
  if (body instanceof Uint8Array) return new TextDecoder().decode(body);
  if (body instanceof ArrayBuffer) return new TextDecoder().decode(body);
  if (ArrayBuffer.isView(body)) {
    return new TextDecoder().decode(new Uint8Array(body.buffer, body.byteOffset, body.byteLength));
  }
  throw new Error(`unexpected request body: ${Object.prototype.toString.call(body)}`);
};

const json = (body: unknown, status = 200): Response =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });

/// The JSON body the client sent.
const sentBody = (call: Call): unknown => {
  if (call.body === null) throw new Error(`${call.method} ${call.url} sent no body`);
  return JSON.parse(call.body);
};

const text = (body: string, status: number): Response =>
  new Response(body, {
    status,
    headers: { "content-type": "text/plain; charset=utf-8" },
  });

const layer = SandboxToolkit.layer({ baseUrl: BASE_URL });

const run = <A, E>(effect: Effect.Effect<A, E, SandboxToolkit>, stub: Stub): Promise<A> =>
  Effect.runPromise(
    effect.pipe(Effect.provide(layer), Effect.provideService(FetchHttpClient.Fetch, stub.fetch)),
  );

const runError = <A>(
  effect: Effect.Effect<A, SandboxToolkitError, SandboxToolkit>,
  stub: Stub,
): Promise<SandboxToolkitError> =>
  Effect.runPromise(
    effect.pipe(
      Effect.provide(layer),
      Effect.provideService(FetchHttpClient.Fetch, stub.fetch),
      Effect.flip,
    ),
  );

const withClient = <A, E>(
  f: (client: SandboxToolkit["Service"]) => Effect.Effect<A, E>,
): Effect.Effect<A, E, SandboxToolkit> =>
  Effect.gen(function* () {
    const client = yield* SandboxToolkit;
    return yield* f(client);
  });

describe("SandboxToolkit", () => {
  it("lists bundled tools", async () => {
    const stub = stubFetch(() =>
      json({
        tools: [
          { name: "fd", path: "/tmp/tools/fd" },
          { name: "rg", path: "/tmp/tools/rg" },
        ],
      }),
    );

    const tools = await run(
      withClient((client) => client.listTools),
      stub,
    );

    expect(tools).toEqual([
      { name: "fd", path: "/tmp/tools/fd" },
      { name: "rg", path: "/tmp/tools/rg" },
    ]);
    expect(stub.calls).toHaveLength(1);
    expect(stub.calls[0].method).toBe("GET");
    expect(stub.calls[0].url).toBe(`${BASE_URL}/tools`);
    expect(stub.calls[0].accept).toBe("application/json");
  });

  it("describes a tool with a JSON request body", async () => {
    const stub = stubFetch(() => json({ name: "rg", path: "/tmp/tools/rg" }));

    const result = await run(
      withClient((client) => client.describeTool({ name: "rg" })),
      stub,
    );

    expect(result).toEqual({ name: "rg", path: "/tmp/tools/rg" });
    expect(stub.calls[0].method).toBe("POST");
    expect(stub.calls[0].url).toBe(`${BASE_URL}/tools/describe`);
    expect(stub.calls[0].contentType).toBe("application/json");
    expect(sentBody(stub.calls[0])).toEqual({ name: "rg" });
  });

  it("reads a window of lines", async () => {
    const stub = stubFetch(() =>
      json({
        path: "/tmp/x.txt",
        lines: [
          { number: 2, text: "bravo" },
          { number: 3, text: "charlie" },
        ],
        truncated: true,
        nextOffset: 3,
      }),
    );

    const result = await run(
      withClient((client) => client.readFile({ path: "/tmp/x.txt", offset: 1, limit: 2 })),
      stub,
    );

    expect(result.lines).toHaveLength(2);
    expect(result.lines[0]).toEqual({ number: 2, text: "bravo" });
    expect(result.truncated).toBe(true);
    expect(result.nextOffset).toBe(3);
    expect(stub.calls[0].url).toBe(`${BASE_URL}/fs/readFile`);
    expect(sentBody(stub.calls[0])).toEqual({
      path: "/tmp/x.txt",
      offset: 1,
      limit: 2,
    });
  });

  it("accepts a response that omits nextOffset", async () => {
    const stub = stubFetch(() =>
      json({
        path: "/tmp/x.txt",
        lines: [{ number: 1, text: "only" }],
        truncated: false,
      }),
    );

    const result = await run(
      withClient((client) => client.readFile({ path: "/tmp/x.txt" })),
      stub,
    );

    expect(result.nextOffset).toBeUndefined();
    expect(result.truncated).toBe(false);
  });

  it("maps 400 to InvalidRequestError", async () => {
    const stub = stubFetch(() => text("path must be absolute: Cargo.toml", 400));

    const error = await runError(
      withClient((client) => client.readFile({ path: "Cargo.toml" })),
      stub,
    );

    expect(error._tag).toBe("SandboxToolkitError");
    expect(error.reason._tag).toBe("InvalidRequestError");
    expect(error.reason.message).toBe("path must be absolute: Cargo.toml");
  });

  it("maps 404 to NotFoundError", async () => {
    const stub = stubFetch(() => text("unknown tool: nope", 404));

    const error = await runError(
      withClient((client) => client.describeTool({ name: "nope" })),
      stub,
    );

    expect(error.reason._tag).toBe("NotFoundError");
    expect(error.reason.message).toBe("unknown tool: nope");
  });

  it("maps other non-2xx statuses to ServerError", async () => {
    const stub = stubFetch(() => text("failed to read file: boom", 500));

    const error = await runError(
      withClient((client) => client.readFile({ path: "/tmp/x.txt" })),
      stub,
    );

    expect(error.reason._tag).toBe("ServerError");
    if (error.reason._tag === "ServerError") {
      expect(error.reason.status).toBe(500);
      expect(error.reason.message).toBe("failed to read file: boom");
    }
  });

  it("maps a network failure to TransportError", async () => {
    const stub: Stub = {
      calls: [],
      fetch: () => Promise.reject(new Error("connection refused")),
    };

    const error = await runError(
      withClient((client) => client.health),
      stub,
    );

    expect(error.reason._tag).toBe("TransportError");
  });

  it("maps an unexpected payload to TransportError", async () => {
    const stub = stubFetch(() => json({ tools: "not-an-array" }));

    const error = await runError(
      withClient((client) => client.listTools),
      stub,
    );

    expect(error.reason._tag).toBe("TransportError");
  });

  it("reads health", async () => {
    const stub = stubFetch(() => json({ status: "ok", uptimeSeconds: 12 }));

    const result = await run(
      withClient((client) => client.health),
      stub,
    );

    expect(result).toEqual({ status: "ok", uptimeSeconds: 12 });
  });
});
