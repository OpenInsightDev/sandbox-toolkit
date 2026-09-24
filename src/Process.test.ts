import { Effect, Layer, Stream } from "effect";
import {
  HttpClient,
  HttpClientError,
  HttpClientRequest,
  HttpClientResponse,
} from "effect/unstable/http";
import { expect, test } from "vite-plus/test";

import type { Status } from "./generated/Status.ts";
import * as Client from "./internal/client.ts";
import * as Process from "./Process.ts";

const baseUrl = "http://sandbox.test";

type Handler = (request: HttpClientRequest.HttpClientRequest, url: URL) => Response | Error;

const stub = (handle: Handler): HttpClient.HttpClient =>
  HttpClient.make((request, url) =>
    Effect.suspend(() => {
      const result = handle(request, url);

      return result instanceof Error
        ? Effect.fail(
            new HttpClientError.HttpClientError({
              reason: new HttpClientError.TransportError({ request, cause: result }),
            }),
          )
        : Effect.succeed(HttpClientResponse.fromWeb(request, result));
    }),
  );

const bodyOf = (request: HttpClientRequest.HttpClientRequest): unknown => {
  const body = request.body;

  return body._tag === "Uint8Array" ? JSON.parse(new TextDecoder().decode(body.body)) : undefined;
};

const result = (status: Status, stdout = "", stderr = "") =>
  Response.json({ status, stdout, stderr });

const execStreamContentType = "application/vnd.sandbox-toolkit.exec-stream";

const encodeText = (text: string): Uint8Array => new TextEncoder().encode(text);

const frame = (channel: number, payload: Uint8Array): Uint8Array => {
  const bytes = new Uint8Array(5 + payload.length);
  bytes[0] = channel;
  bytes[1] = (payload.length >>> 24) & 0xff;
  bytes[2] = (payload.length >>> 16) & 0xff;
  bytes[3] = (payload.length >>> 8) & 0xff;
  bytes[4] = payload.length & 0xff;
  bytes.set(payload, 5);

  return bytes;
};

const statusFrame = (status: Status): Uint8Array => frame(2, encodeText(JSON.stringify(status)));

const streamResponse = (...chunks: ReadonlyArray<Uint8Array>): Response =>
  new Response(
    new ReadableStream<Uint8Array>({
      start(controller) {
        for (const chunk of chunks) {
          controller.enqueue(chunk);
        }
        controller.close();
      },
    }),
    { headers: { "content-type": execStreamContentType } },
  );

const decoded = (events: ReadonlyArray<Process.ProcessEvent>) =>
  events.map((event) =>
    event._tag === "Exit"
      ? { _tag: event._tag, exitCode: event.exitCode }
      : { _tag: event._tag, data: new TextDecoder().decode(event.data) },
  );

const layerOf = (workspace: string | undefined, handle: Handler) => {
  const client = Client.layer({ baseUrl }).pipe(
    Layer.provide(Layer.succeed(HttpClient.HttpClient, stub(handle))),
  );
  const process =
    workspace === undefined ? Process.layer : Process.layerForWorkspace({ workspace });

  return process.pipe(Layer.provide(client));
};

const run = <A, E>(
  program: Effect.Effect<A, E, Process.Process>,
  workspace: string | undefined,
  handle: Handler,
): Promise<A> => Effect.runPromise(Effect.provide(program, layerOf(workspace, handle)));

test("runs an exec in the workspace and returns its stdout", async () => {
  const requests: Array<{ url: string; body: unknown }> = [];

  const program = Effect.gen(function* () {
    const process = yield* Process.Process;

    return yield* process.string({ command: "echo", args: ["hi"] });
  });

  const value = await run(program, "docs", (request, url) => {
    requests.push({ url: url.toString(), body: bodyOf(request) });

    return result({ status: "success" }, "hi\n");
  });

  expect(value).toBe("hi\n");
  expect(requests).toEqual([
    {
      url: "http://sandbox.test/workspaces/docs/exec",
      body: { command: "echo", args: ["hi"], cwd: null, env: {}, timeout: null },
    },
  ]);
});

test("runs an exec in direct mode at the root route", async () => {
  let seen: string | undefined;

  const program = Effect.gen(function* () {
    const process = yield* Process.Process;

    return yield* process.exitCode({ command: "true", args: [] });
  });

  const value = await run(program, undefined, (_request, url) => {
    seen = url.toString();

    return result({ status: "exited", code: 3 });
  });

  expect(seen).toBe("http://sandbox.test/exec");
  expect(value).toBe(3);
});

test("splits the output into lines", async () => {
  const program = Effect.gen(function* () {
    const process = yield* Process.Process;

    return yield* Stream.runCollect(process.lines({ command: "ls", args: [] }));
  });

  const value = await run(program, "docs", () => result({ status: "success" }, "a\nb\n"));

  expect(Array.from(value)).toEqual(["a", "b"]);
});

test("streams a trailing line that has no line ending", async () => {
  const program = Effect.gen(function* () {
    const process = yield* Process.Process;

    return yield* Stream.runCollect(process.lines({ command: "ls", args: [] }));
  });

  const value = await run(program, "docs", () => result({ status: "success" }, "a\r\nb\nlast"));

  expect(Array.from(value)).toEqual(["a", "b", "last"]);
});

test("folds stderr into the output when asked", async () => {
  const program = Effect.gen(function* () {
    const process = yield* Process.Process;

    return yield* process.string({ command: "ls", args: [] }, { includeStderr: true });
  });

  const value = await run(program, "docs", () => result({ status: "success" }, "out", "err"));

  expect(value).toBe("outerr");
});

test("reports a command that could not run as CommandFailed", async () => {
  const program = Effect.gen(function* () {
    const process = yield* Process.Process;

    return yield* process.exitCode({ command: "missing", args: [] });
  });

  await expect(
    run(program, "docs", () => result({ status: "failed", message: "spawn failed" })),
  ).rejects.toBeInstanceOf(Process.CommandFailed);
});

test("runs a shell template with its options", async () => {
  let body: unknown;

  const program = Effect.gen(function* () {
    const process = yield* Process.Process;

    return yield* process.$({ cwd: "sub" })`echo ${"hi"}`;
  });

  const value = await run(program, "docs", (request, url) => {
    expect(url.toString()).toBe("http://sandbox.test/workspaces/docs/shell");
    body = bodyOf(request);

    return result({ status: "success" }, "hi\n");
  });

  expect(value).toBe("hi\n");
  expect(body).toEqual({
    script: "echo hi",
    cwd: "sub",
    env: {},
    shell: null,
    timeout: null,
  });
});

const collectStream = (
  command: Process.Command,
): Effect.Effect<ReadonlyArray<Process.ProcessEvent>, Process.ProcessError, Process.Process> =>
  Effect.flatMap(Process.Process, (process) => Stream.runCollect(process.stream(command)));

test("decodes the multiplexed frame stream into events", async () => {
  let body: unknown;

  const chunk = await run(
    collectStream({ command: "ls", args: ["-l"], options: { cwd: "sub" } }),
    "docs",
    (request, url) => {
      expect(url.toString()).toBe("http://sandbox.test/workspaces/docs/exec");
      body = bodyOf(request);

      return streamResponse(
        frame(0, encodeText("out")),
        frame(1, encodeText("err")),
        statusFrame({ status: "exited", code: 2 }),
      );
    },
  );

  expect(decoded(chunk)).toEqual([
    { _tag: "Stdout", data: "out" },
    { _tag: "Stderr", data: "err" },
    { _tag: "Exit", exitCode: 2 },
  ]);
  expect(body).toEqual({ command: "ls", args: ["-l"], cwd: "sub", env: {}, timeout: 0 });
});

test("reassembles frames split across chunks", async () => {
  const stdout = frame(0, encodeText("hello"));
  const status = statusFrame({ status: "success" });

  const chunk = await run(collectStream({ command: "ls", args: [] }), "docs", () =>
    streamResponse(stdout.slice(0, 3), stdout.slice(3), status.slice(0, 2), status.slice(2)),
  );

  expect(decoded(chunk)).toEqual([
    { _tag: "Stdout", data: "hello" },
    { _tag: "Exit", exitCode: 0 },
  ]);
});

test("synthesizes events from a direct result", async () => {
  const chunk = await run(collectStream({ command: "ls", args: [] }), "docs", () =>
    result({ status: "exited", code: 5 }, "out", "err"),
  );

  expect(decoded(chunk)).toEqual([
    { _tag: "Stdout", data: "out" },
    { _tag: "Stderr", data: "err" },
    { _tag: "Exit", exitCode: 5 },
  ]);
});

test("fails the stream when the command could not run", async () => {
  await expect(
    run(collectStream({ command: "missing", args: [] }), "docs", () =>
      result({ status: "failed", message: "spawn failed" }),
    ),
  ).rejects.toBeInstanceOf(Process.CommandFailed);
});

test("fails the stream when it ends without a status frame", async () => {
  await expect(
    run(collectStream({ command: "ls", args: [] }), "docs", () =>
      streamResponse(frame(0, encodeText("out"))),
    ),
  ).rejects.toBeInstanceOf(Process.StreamError);
});

test("fails the stream on a malformed status frame", async () => {
  await expect(
    run(collectStream({ command: "ls", args: [] }), "docs", () =>
      streamResponse(frame(2, encodeText("not json"))),
    ),
  ).rejects.toBeInstanceOf(Process.StreamError);
});

test("collects the frame stream into a result", async () => {
  const program = Effect.gen(function* () {
    const process = yield* Process.Process;
    const spawned = yield* process.result({ command: "ls", args: [] });

    return {
      stdout: yield* Stream.mkString(Stream.decodeText(spawned.stdout)),
      stderr: yield* Stream.mkString(Stream.decodeText(spawned.stderr)),
      exitCode: yield* spawned.exitCode,
    };
  });

  const value = await run(program, "docs", () =>
    streamResponse(
      frame(0, encodeText("out")),
      frame(1, encodeText("err")),
      statusFrame({ status: "exited", code: 4 }),
    ),
  );

  expect(value).toEqual({ stdout: "out", stderr: "err", exitCode: 4 });
});

test("exec hands back a direct result when the server answers inline", async () => {
  const program = Effect.gen(function* () {
    const process = yield* Process.Process;
    const outcome = yield* process.exec({ command: "echo", args: ["hi"] });

    return Stream.isStream(outcome)
      ? { shape: "stream" as const }
      : {
          shape: "result" as const,
          stdout: yield* Stream.mkString(Stream.decodeText(outcome.stdout)),
          exitCode: yield* outcome.exitCode,
        };
  });

  const value = await run(program, "docs", () => result({ status: "exited", code: 2 }, "out"));

  expect(value).toEqual({ shape: "result", stdout: "out", exitCode: 2 });
});

test("exec hands back the frame stream when the server upgrades", async () => {
  const program = Effect.gen(function* () {
    const process = yield* Process.Process;
    const outcome = yield* process.exec({ command: "ls", args: [] });

    return Stream.isStream(outcome) ? decoded(yield* Stream.runCollect(outcome)) : undefined;
  });

  const value = await run(program, "docs", () =>
    streamResponse(
      frame(0, encodeText("out")),
      frame(1, encodeText("err")),
      statusFrame({ status: "exited", code: 2 }),
    ),
  );

  expect(value).toEqual([
    { _tag: "Stdout", data: "out" },
    { _tag: "Stderr", data: "err" },
    { _tag: "Exit", exitCode: 2 },
  ]);
});
