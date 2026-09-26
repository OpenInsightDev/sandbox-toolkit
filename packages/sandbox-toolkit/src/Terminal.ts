import { Context, Effect, Layer, Option, PlatformError, Predicate, Queue } from "effect";
import type { Cause, Scope } from "effect";
import { QuitError, type UserInput } from "effect/Terminal";
import { HttpClientRequest } from "effect/unstable/http";
import * as Socket from "effect/unstable/socket/Socket";

import type { PtyRequest } from "./generated/PtyRequest.ts";
import type { PtySession } from "./generated/PtySession.ts";
import { layer as http2WebSocket } from "./internal/Http2WebSocket.ts";
import { Client } from "./internal/client.ts";
import { endLines, takeLines } from "./internal/process.ts";
import { route, targetEnv } from "./internal/prelude.ts";
import { Frame, decodeFrame, encodeFrame, webSocketUrl } from "./internal/terminal.ts";
import type { WorkspaceHandle } from "./Workspace.ts";

export interface Terminal {
  /**
   * The number of columns available on the platform's terminal interface.
   */
  readonly columns: Effect.Effect<number>;
  /**
   * The number of rows available on the platform's terminal interface.
   */

  readonly rows: Effect.Effect<number>;
  /**
   * Reads input events from the default standard input.
   */
  readonly readInput: Effect.Effect<Queue.Dequeue<UserInput, Cause.Done>, never, Scope.Scope>;
  /**
   * Reads a single line from the default standard input.
   */
  readonly readLine: Effect.Effect<string, QuitError>;
  /**
   * Displays text to the default standard output.
   */
  readonly display: (text: string) => Effect.Effect<void, PlatformError.PlatformError>;
}

export const Terminal: Context.Service<Terminal, Terminal> = Context.Service("effect/Terminal");

export interface TerminalOptions {
  /**
   * The program the session runs, resolved through `PATH`; `sh` when omitted.
   */
  readonly command?: string | undefined;
  readonly args?: ReadonlyArray<string> | undefined;
  /**
   * Workspace-relative in workspace mode, absolute in direct mode.
   */
  readonly cwd?: string | undefined;
  readonly env?: Record<string, string | undefined> | undefined;
  /**
   * The size the session starts at; the runtime default when omitted.
   */
  readonly size?: { readonly rows: number; readonly columns: number } | undefined;
}

const DEFAULT_COMMAND = "sh";

const DEFAULT_SIZE = { rows: 24, columns: 80 };

/**
 * A `Terminal` is backed by one pty session, created when the layer is built
 * and torn down with its scope. The mapping onto the platform interface treats
 * the session as the terminal device:
 *
 * - `display` writes a `stdin` frame, and
 * - `readInput` / `readLine` consume the `stdout` frames.
 *
 * A pty carries no key metadata, so each decoded chunk is one input event and
 * its text is surfaced as-is.
 */
export const make = Effect.fn("Terminal.make")(function* (
  options: { workspace?: string | undefined } & TerminalOptions = {},
) {
  const client = yield* Client;

  const size = options.size ?? DEFAULT_SIZE;

  const request: PtyRequest = {
    command: options.command ?? DEFAULT_COMMAND,
    args: [...(options.args ?? [])],
    cwd: options.cwd ?? null,
    env: targetEnv(options.env),
    size: { rows: size.rows, cols: size.columns },
  };

  const session = yield* client.json<PtySession>(
    HttpClientRequest.post(route(options.workspace, "/pty")).pipe(
      HttpClientRequest.bodyJsonUnsafe(request),
    ),
  );

  const attach =
    session.endpoint === "" ? route(options.workspace, `/pty/${session.id}`) : session.endpoint;

  const socket = yield* Socket.makeWebSocket(webSocketUrl(client.baseUrl, attach));
  const pull = yield* Socket.readerBytes(socket);
  const writer = yield* socket.writer;

  const inputs = yield* Queue.make<UserInput, Cause.Done>();
  const lines = yield* Queue.make<string, Cause.Done>();

  const key = { name: "", ctrl: false, meta: false, shift: false } as const;

  const consume = Effect.gen(function* () {
    const decoder = new TextDecoder();
    let buffer = "";
    let done = false;

    const emit = (text: string) =>
      Effect.gen(function* () {
        if (text !== "") {
          yield* Queue.offer(inputs, { input: Option.some(text), key });
        }

        const [next, complete] = takeLines(buffer, text);
        buffer = next;

        yield* Effect.forEach(complete, (line) => Queue.offer(lines, line), { discard: true });
      });

    const finish = () =>
      Effect.gen(function* () {
        if (done) return;

        yield* emit(decoder.decode());
        yield* Effect.forEach(endLines(buffer), (line) => Queue.offer(lines, line), {
          discard: true,
        });
        yield* Queue.end(inputs);
        yield* Queue.end(lines);
        done = true;
      });

    while (!done) {
      const frames = yield* pull;

      for (const frame of frames) {
        const decoded = Option.getOrUndefined(decodeFrame(frame));

        if (decoded === undefined) continue;

        if (Predicate.isTagged("Stdout")(decoded)) {
          yield* emit(decoder.decode(decoded.data, { stream: true }));
        } else if (Predicate.isTagged("Exit")(decoded)) {
          yield* finish();
        }
      }
    }
  });

  yield* Effect.forkScoped(
    consume.pipe(
      Effect.catchCause(() => Effect.all([Queue.end(inputs), Queue.end(lines)], { discard: true })),
    ),
  );

  const readInput: Terminal["readInput"] = Effect.succeed(inputs);

  const readLine: Terminal["readLine"] = Effect.suspend(() =>
    Queue.poll(lines).pipe(
      Effect.flatMap(
        Option.match({
          onNone: () => Queue.take(lines),
          onSome: Effect.succeed,
        }),
      ),
      Effect.mapError(() => new QuitError({})),
    ),
  );

  const display = ((text: string) =>
    Effect.uninterruptible(
      writer.write(encodeFrame(Frame.Stdin({ data: new TextEncoder().encode(text) }))).pipe(
        Effect.mapError((error) =>
          PlatformError.badArgument({
            module: "Terminal",
            method: "display",
            description: error.message,
            cause: error,
          }),
        ),
      ),
    )) satisfies Terminal["display"];

  return Terminal.of({
    columns: Effect.succeed(size.columns),
    rows: Effect.succeed(size.rows),
    readInput,
    readLine,
    display,
  });
});

/**
 * The terminal service over a workspace, where `cwd` is a workspace-relative
 * path.
 */
export const layerForWorkspace = ({
  workspace,
  ...options
}: { workspace: WorkspaceHandle } & TerminalOptions) =>
  Layer.effect(Terminal, make({ workspace: workspace.id, ...options })).pipe(
    Layer.provide(http2WebSocket),
  );

/** The terminal service in direct mode, where `cwd` is an absolute path. */
export const layer = (options: TerminalOptions = {}) =>
  Layer.effect(Terminal, make(options)).pipe(Layer.provide(http2WebSocket));
