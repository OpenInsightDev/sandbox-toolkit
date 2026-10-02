import { Effect, Layer, Option, Queue } from "effect";
import {
  HttpClient,
  HttpClientError,
  HttpClientRequest,
  HttpClientResponse,
} from "effect/unstable/http";
import * as Socket from "effect/unstable/socket/Socket";
import { QuitError } from "effect/Terminal";
import { expect, test } from "vite-plus/test";

import * as Client from "../src/internal/client.ts";
import { Frame, decodeFrame, encodeFrame } from "../src/internal/terminal.ts";
import * as Terminal from "../src/Terminal.ts";

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

type Listener = (event: Socket.WebSocketEvent) => void;

/** A single WebSocket the test drives from either end. */
class FakeSocket implements Socket.WebSocketLike {
  readyState = 1;
  readonly sent: Array<Uint8Array | string> = [];

  readonly #listeners = new Map<string, Set<Listener>>();

  addEventListener(type: string, listener: Listener, _options?: { readonly once?: boolean }): void {
    const listeners = this.#listeners.get(type) ?? new Set<Listener>();
    listeners.add(listener);
    this.#listeners.set(type, listeners);
  }

  removeEventListener(type: string, listener: Listener): void {
    this.#listeners.get(type)?.delete(listener);
  }

  close(code?: number, reason?: string): void {
    this.readyState = 3;
    this.emit("close", { code, reason });
  }

  send(data: string | Uint8Array<ArrayBuffer>): void {
    this.sent.push(data);
  }

  emit(type: string, event: Socket.WebSocketEvent): void {
    for (const listener of this.#listeners.get(type) ?? []) {
      listener(event);
    }
  }

  /** Feeds one frame as if it came off the socket. */
  receive(frame: Uint8Array): void {
    this.emit("message", { data: frame });
  }

  /** The frames the service wrote, decoded. */
  frames(): ReadonlyArray<Frame> {
    return this.sent.flatMap((data) => {
      const bytes = typeof data === "string" ? new TextEncoder().encode(data) : data;

      return Option.match(decodeFrame(bytes), { onNone: () => [], onSome: (frame) => [frame] });
    });
  }
}

const layerOf = (
  options: { workspace?: string } & Terminal.TerminalOptions,
  handle: Handler,
  socket: FakeSocket,
  urls: Array<string> = [],
) => {
  const client = Client.layer({ baseUrl }).pipe(
    Layer.provide(Layer.succeed(HttpClient.HttpClient, stub(handle))),
  );

  return Layer.effect(Terminal.Terminal, Terminal.make(options)).pipe(
    Layer.provide(client),
    Layer.provide(
      Layer.succeed(Socket.WebSocketConstructor, (url) => {
        urls.push(url);

        return socket;
      }),
    ),
  );
};

const run = <A, E>(
  program: Effect.Effect<A, E, Terminal.Terminal>,
  options: { workspace?: string } & Terminal.TerminalOptions,
  handle: Handler,
  socket: FakeSocket,
  urls: Array<string> = [],
): Promise<A> => Effect.runPromise(Effect.provide(program, layerOf(options, handle, socket, urls)));

const session = (endpoint = "/pty/s1") => Response.json({ id: "s1", endpoint });

test("creates the session in the workspace and reports the configured size", async () => {
  const socket = new FakeSocket();
  const requests: Array<{ url: string; body: unknown }> = [];

  const program = Effect.gen(function* () {
    const terminal = yield* Terminal.Terminal;

    return { columns: yield* terminal.columns, rows: yield* terminal.rows };
  });

  const value = await run(
    program,
    { workspace: "docs", size: { rows: 30, columns: 100 }, args: ["-l"], cwd: "sub" },
    (request, url) => {
      requests.push({ url: url.toString(), body: bodyOf(request) });

      return session();
    },
    socket,
  );

  expect(value).toEqual({ columns: 100, rows: 30 });
  expect(requests).toEqual([
    {
      url: "http://sandbox.test/workspaces/docs/pty",
      body: {
        command: "sh",
        args: ["-l"],
        cwd: "sub",
        env: {},
        size: { rows: 30, cols: 100 },
      },
    },
  ]);
});

test("creates the session at the root route in direct mode", async () => {
  const socket = new FakeSocket();
  const urls: Array<string> = [];
  let seen: string | undefined;

  await run(
    Effect.gen(function* () {
      yield* Terminal.Terminal;
    }),
    { command: "bash" },
    (request, url) => {
      seen = url.toString();

      return session();
    },
    socket,
    urls,
  );

  expect(seen).toBe("http://sandbox.test/pty");
  expect(urls).toEqual(["ws://sandbox.test/pty/s1"]);
});

test("connects to the endpoint the server returned", async () => {
  const socket = new FakeSocket();
  const urls: Array<string> = [];

  await run(
    Effect.gen(function* () {
      yield* Terminal.Terminal;
    }),
    { workspace: "docs" },
    () => session("/workspaces/docs/pty/s1"),
    socket,
    urls,
  );

  expect(urls).toEqual(["ws://sandbox.test/workspaces/docs/pty/s1"]);
});

test("displays text as a stdin frame", async () => {
  const socket = new FakeSocket();

  await run(
    Effect.gen(function* () {
      const terminal = yield* Terminal.Terminal;

      yield* terminal.display("ls\n");
    }),
    {},
    () => session(),
    socket,
  );

  expect(socket.frames()).toEqual([Frame.Stdin({ data: new TextEncoder().encode("ls\n") })]);
});

test("reads lines from the stdout frames and ends on exit", async () => {
  const socket = new FakeSocket();

  const program = Effect.gen(function* () {
    const terminal = yield* Terminal.Terminal;

    yield* Effect.sync(() => {
      socket.receive(encodeFrame(Frame.Stdout({ data: new TextEncoder().encode("a\nb\n") })));
      socket.receive(encodeFrame(Frame.Exit({ status: { status: "exited", exit_code: 0 } })));
    });

    const first = yield* terminal.readLine;
    const second = yield* terminal.readLine;
    const quit = yield* Effect.flip(terminal.readLine);

    return { first, second, quit };
  });

  const value = await run(program, {}, () => session(), socket);

  expect(value.first).toBe("a");
  expect(value.second).toBe("b");
  expect(value.quit).toBeInstanceOf(QuitError);
});

test("surfaces each stdout chunk as one input event", async () => {
  const socket = new FakeSocket();

  const program = Effect.gen(function* () {
    const terminal = yield* Terminal.Terminal;
    const inputs = yield* Effect.scoped(terminal.readInput);

    yield* Effect.sync(() => {
      socket.receive(encodeFrame(Frame.Stdout({ data: new TextEncoder().encode("hi") })));
      socket.receive(encodeFrame(Frame.Exit({ status: { status: "exited", exit_code: 0 } })));
    });

    return yield* Queue.take(inputs);
  });

  const value = await run(program, {}, () => session(), socket);

  expect(Option.getOrUndefined(value.input)).toBe("hi");
});

test("encodes and decodes every frame", () => {
  const frames = [
    Frame.Stdin({ data: new Uint8Array([1, 2]) }),
    Frame.Stdout({ data: new Uint8Array([3]) }),
    Frame.Exit({ status: { status: "exited", exit_code: 3 } }),
    Frame.Resize({ size: { rows: 24, cols: 80 } }),
  ];

  for (const frame of frames) {
    expect(Option.getOrUndefined(decodeFrame(encodeFrame(frame)))).toEqual(frame);
  }
});

test("rejects an unknown channel and a short resize", () => {
  expect(Option.isNone(decodeFrame(new Uint8Array([2])))).toBe(true);
  expect(Option.isNone(decodeFrame(new Uint8Array([4, 0, 0])))).toBe(true);
  expect(Option.isNone(decodeFrame(new Uint8Array([])))).toBe(true);
  // 255 was the in-band close channel; closing is the WebSocket close frame.
  expect(Option.isNone(decodeFrame(new Uint8Array([255])))).toBe(true);
});
