import { Data, Effect, Match, Option, Record, Ref, Schema, Stream } from "effect";
import type { HttpClientResponse } from "effect/unstable/http";
import type { TemplateExpression } from "effect/unstable/process/ChildProcess";
import { ExitCode } from "effect/unstable/process/ChildProcessSpawner";

import type { Command, ProcessResult, ShellCommandOptions } from "../Process.ts";
import type { ExecRequest } from "../generated/ExecRequest.ts";
import type { ExecResult } from "../generated/ExecResult.ts";
import type { ShellRequest } from "../generated/ShellRequest.ts";
import type { Status } from "../generated/Status.ts";
import { transportError, type ClientError } from "./client.ts";

const targetEnv = (env: Record<string, string | undefined> | undefined): Record<string, string> =>
  Record.filter(env ?? {}, (value): value is string => value !== undefined);

export const execRequest = (command: Command): ExecRequest => ({
  command: command.command,
  args: [...command.args],
  cwd: command.options?.cwd ?? null,
  env: targetEnv(command.options?.env),
  timeout: command.options?.timeout ?? null,
});

export const shellRequest = (
  script: string,
  options: ShellCommandOptions | undefined,
): ShellRequest => ({
  script,
  cwd: options?.cwd ?? null,
  env: targetEnv(options?.env),
  shell: options?.shell ?? null,
  timeout: options?.timeout ?? null,
});

/**
 * Interpolated values are inlined as written; the caller owns any quoting,
 * because the interpreter receives the rendered script verbatim.
 */
export const renderShell = (
  templates: TemplateStringsArray,
  values: ReadonlyArray<TemplateExpression>,
): string =>
  templates.reduce(
    (script, chunk, index) =>
      script + chunk + (index < values.length ? shellWord(values[index]) : ""),
    "",
  );

const shellWord = (value: TemplateExpression): string =>
  Array.isArray(value) ? value.map((item) => String(item)).join(" ") : String(value);

export const isTemplateStrings = (
  value: TemplateStringsArray | ShellCommandOptions,
): value is TemplateStringsArray => Array.isArray(value);

/**
 * Splits `buffer + chunk` into the lines it terminates, keeping the unterminated
 * tail as the next buffer. A trailing `\r` is held back since the `\n` that
 * would complete a `\r\n` separator may arrive in a later chunk.
 */
export const takeLines = (
  buffer: string,
  chunk: string,
): readonly [string, ReadonlyArray<string>] => {
  const text = buffer + chunk;
  const lines: Array<string> = [];
  let start = 0;

  for (let index = 0; index < text.length; index++) {
    const char = text[index];

    if (char !== "\n" && char !== "\r") continue;

    if (char === "\r" && index + 1 === text.length) break;

    lines.push(text.slice(start, index));
    index += char === "\r" && text[index + 1] === "\n" ? 1 : 0;
    start = index + 1;
  }

  return [text.slice(start), lines];
};

/**
 * Completes the final line at the end of the stream. A trailing `\r` is a held
 * separator rather than content, and a separator at the very end contributes no
 * extra empty line.
 */
export const endLines = (buffer: string): ReadonlyArray<string> =>
  buffer === "" ? [] : [buffer.endsWith("\r") ? buffer.slice(0, -1) : buffer];

/** A command that could not run, or was terminated before it could exit. */
export class CommandFailed extends Data.TaggedError("CommandFailed")<{
  readonly message: string;
}> {}

/** The exec frame stream violated its wire format. */
export class StreamError extends Data.TaggedError("StreamError")<{
  readonly message: string;
}> {}

/**
 * A shell command that ran to completion with a non-zero exit code, carrying
 * the script and both output streams so a failing command is diagnosable.
 */
export class CommandExitError extends Data.TaggedError("CommandExitError")<{
  readonly script: string;
  readonly exitCode: number;
  readonly stdout: string;
  readonly stderr: string;
}> {}

export type ProcessError = ClientError | CommandFailed | StreamError | CommandExitError;

/**
 * One message read from the exec frame stream, tagged by channel.
 *
 * A run emits any number of `Stdout` and `Stderr` chunks, then exactly one
 * `Exit` carrying the command's exit code.
 */
export type ProcessEvent = Data.TaggedEnum<{
  Stdout: { readonly data: Uint8Array };
  Stderr: { readonly data: Uint8Array };
  Exit: { readonly exitCode: ExitCode };
}>;

export const ProcessEvent = Data.taggedEnum<ProcessEvent>();

/** One message on the exec frame stream, before the status is turned into an exit code. */
type Frame = Data.TaggedEnum<{
  Stdout: { readonly data: Uint8Array };
  Stderr: { readonly data: Uint8Array };
  Status: { readonly status: Status };
}>;

const Frame = Data.taggedEnum<Frame>();

/** A frame header: one channel byte and a four-byte big-endian length. */
const HEADER_LEN = 5;

/** The largest payload a frame may carry, mirroring the server's bound. */
const MAX_PAYLOAD_LEN = 4 * 1024 * 1024;

const STDOUT = 0;

const STDERR = 1;

const ERROR = 2;

const EMPTY = new Uint8Array(0);

const statusSchema = Schema.fromJsonString(
  Schema.Union([
    Schema.Struct({ status: Schema.Literal("success") }),
    Schema.Struct({ status: Schema.Literal("exited"), code: Schema.Number }),
    Schema.Struct({ status: Schema.Literal("failed"), message: Schema.String }),
  ]),
);

const parseStatus = (payload: Uint8Array): Status | undefined =>
  Option.getOrUndefined(
    Schema.decodeUnknownOption(statusSchema)(new TextDecoder().decode(payload)),
  );

/** Joins byte chunks into a single buffer. */
const concat = (chunks: ReadonlyArray<Uint8Array>): Uint8Array => {
  const total = chunks.reduce((length, chunk) => length + chunk.length, 0);
  const merged = new Uint8Array(total);
  let offset = 0;

  for (const chunk of chunks) {
    merged.set(chunk, offset);
    offset += chunk.length;
  }

  return merged;
};

/** Bytes read so far, plus whether the terminal status frame has arrived. */
interface DecodeState {
  readonly buffer: Uint8Array;
  readonly done: boolean;
}

type Decoded = Data.TaggedEnum<{
  Ok: { readonly state: DecodeState; readonly frames: ReadonlyArray<Frame> };
  Error: { readonly message: string };
}>;

const Decoded = Data.taggedEnum<Decoded>();

/** Consumes one chunk, returning the frames it completed and the bytes left over. */
const decodeChunk = (state: DecodeState, chunk: Uint8Array): Decoded => {
  if (state.done) {
    return chunk.length === 0
      ? Decoded.Ok({ state, frames: [] })
      : Decoded.Error({ message: "frame received after the terminal status frame" });
  }

  let buffer = concat([state.buffer, chunk]);
  const frames: Array<Frame> = [];

  while (buffer.length >= HEADER_LEN) {
    const channel = buffer[0];
    const length = ((buffer[1] << 24) | (buffer[2] << 16) | (buffer[3] << 8) | buffer[4]) >>> 0;

    if (channel !== STDOUT && channel !== STDERR && channel !== ERROR) {
      return Decoded.Error({ message: `unknown channel id: ${channel}` });
    }

    if (length > MAX_PAYLOAD_LEN) {
      return Decoded.Error({
        message: `frame payload of ${length} bytes exceeds the per-frame limit`,
      });
    }

    if (buffer.length < HEADER_LEN + length) {
      break;
    }

    const payload = buffer.slice(HEADER_LEN, HEADER_LEN + length);
    buffer = buffer.slice(HEADER_LEN + length);

    if (channel === ERROR) {
      const status = parseStatus(payload);

      if (status === undefined) {
        return Decoded.Error({ message: "status frame payload is not a status" });
      }

      frames.push(Frame.Status({ status }));

      return buffer.length === 0
        ? Decoded.Ok({ state: { buffer: EMPTY, done: true }, frames })
        : Decoded.Error({ message: "frame received after the terminal status frame" });
    }

    frames.push(
      channel === STDOUT ? Frame.Stdout({ data: payload }) : Frame.Stderr({ data: payload }),
    );
  }

  return Decoded.Ok({ state: { buffer, done: false }, frames });
};

/**
 * Decodes the length-prefixed exec frames from a byte stream, tolerating frames
 * split across chunks. Fails when the stream is malformed, carries a frame after
 * its terminal status, or ends without one.
 */
const decodeFrames = <E>(
  bytes: Stream.Stream<Uint8Array, E>,
): Stream.Stream<Frame, E | StreamError> =>
  Stream.unwrap(
    Effect.gen(function* () {
      const state = yield* Ref.make<DecodeState>({ buffer: EMPTY, done: false });

      return bytes.pipe(
        Stream.mapAccumEffect(
          () => undefined,
          (_void, chunk) =>
            Effect.flatMap(Ref.get(state), (current) => {
              const decoded = decodeChunk(current, chunk);

              return Decoded.$match(decoded, {
                Error: ({ message }) => Effect.fail(new StreamError({ message })),
                Ok: ({ state: next, frames }) =>
                  Effect.as(Ref.set(state, next), [undefined, frames] as const),
              });
            }),
        ),
        Stream.onEnd(
          Effect.flatMap(Ref.get(state), ({ done }) =>
            done
              ? Effect.void
              : Effect.fail(
                  new StreamError({ message: "the exec stream ended without a status frame" }),
                ),
          ),
        ),
      );
    }),
  );

/** Media type of the upgraded exec response: the length-prefixed frame stream. */
const EXEC_STREAM_CONTENT_TYPE = "application/vnd.sandbox-toolkit.exec-stream";

const exitCodeOf = (status: Status): Effect.Effect<ExitCode, CommandFailed> =>
  Match.value(status).pipe(
    Match.discriminatorsExhaustive("status")({
      success: () => Effect.succeed(ExitCode(0)),
      exited: ({ code }) => Effect.succeed(ExitCode(code)),
      failed: ({ message }) => Effect.fail(new CommandFailed({ message })),
    }),
  );

const { $match } = Frame;

const frameToEvent = (frame: Frame): Effect.Effect<ProcessEvent, CommandFailed> =>
  $match(frame, {
    Stdout: ({ data }) => Effect.succeed(ProcessEvent.Stdout({ data })),
    Stderr: ({ data }) => Effect.succeed(ProcessEvent.Stderr({ data })),
    Status: ({ status }) =>
      Effect.map(exitCodeOf(status), (exitCode) => ProcessEvent.Exit({ exitCode })),
  });

const textBytes = (text: string): Uint8Array => new TextEncoder().encode(text);

/** Replays a direct result as the events the frame stream would have carried. */
const directEvents = (result: ExecResult): Stream.Stream<ProcessEvent, CommandFailed> => {
  const events: Array<ProcessEvent> = [];

  if (result.stdout !== "") {
    events.push(ProcessEvent.Stdout({ data: textBytes(result.stdout) }));
  }

  if (result.stderr !== "") {
    events.push(ProcessEvent.Stderr({ data: textBytes(result.stderr) }));
  }

  return Stream.concat(
    Stream.fromArray(events),
    Stream.fromEffect(
      Effect.map(exitCodeOf(result.status), (exitCode) => ProcessEvent.Exit({ exitCode })),
    ),
  );
};

const isExecStream = (response: HttpClientResponse.HttpClientResponse): boolean =>
  (response.headers["content-type"] ?? "").includes(EXEC_STREAM_CONTENT_TYPE);

// SAFETY: the exec endpoint answers with the shared `ExecResult` envelope, so the
// decoded JSON body is that result; its fields are read only by callers that
// requested the matching response shape.
const jsonBody = (
  response: HttpClientResponse.HttpClientResponse,
): Effect.Effect<ExecResult, ProcessError> =>
  Effect.map(Effect.mapError(response.json, transportError), (body) => body as ExecResult);

/**
 * Decodes whichever shape the server answered with into the multiplexed event
 * stream: the frame stream when it upgraded the response, the direct result
 * otherwise.
 */
export const responseEvents = (
  response: HttpClientResponse.HttpClientResponse,
): Stream.Stream<ProcessEvent, ProcessError> =>
  isExecStream(response)
    ? Stream.mapEffect(decodeFrames(Stream.mapError(response.stream, transportError)), frameToEvent)
    : Stream.unwrap(Effect.map(jsonBody(response), directEvents));

const { $is } = ProcessEvent;

/** Collapses the multiplexed events into the single final result of a collection. */
const collectedResult = (events: ReadonlyArray<ProcessEvent>): ProcessResult => {
  // `decodeFrames` guarantees a terminal `Exit`, so the fallback is unreachable.
  const exitCode = events.find($is("Exit"))?.exitCode ?? ExitCode(0);

  return {
    exitCode: Effect.succeed(exitCode),
    stdout: Stream.make(
      concat(events.flatMap((event) => ($is("Stdout")(event) ? [event.data] : []))),
    ),
    stderr: Stream.make(
      concat(events.flatMap((event) => ($is("Stderr")(event) ? [event.data] : []))),
    ),
  };
};

/** The collected result of a command the server answered directly. */
const directResult = (result: ExecResult): Effect.Effect<ProcessResult, CommandFailed> =>
  Effect.map(exitCodeOf(result.status), (exitCode) => ({
    exitCode: Effect.succeed(exitCode),
    stdout: Stream.make(textBytes(result.stdout)),
    stderr: Stream.make(textBytes(result.stderr)),
  }));

/**
 * Collects whichever shape the server answered with into the final result: a
 * direct result is used as-is, the frame stream is drained first.
 */
export const responseResult = (
  response: HttpClientResponse.HttpClientResponse,
): Effect.Effect<ProcessResult, ProcessError> =>
  isExecStream(response)
    ? Effect.map(Stream.runCollect(responseEvents(response)), collectedResult)
    : Effect.flatMap(jsonBody(response), directResult);

/**
 * Hands back whichever shape the server answered with as-is: the collected
 * result for a direct response, the event stream for an upgraded one.
 */
export const responseExec = (
  response: HttpClientResponse.HttpClientResponse,
): Effect.Effect<ProcessResult | Stream.Stream<ProcessEvent, ProcessError>, ProcessError> =>
  isExecStream(response)
    ? Effect.succeed(responseEvents(response))
    : Effect.flatMap(jsonBody(response), directResult);
