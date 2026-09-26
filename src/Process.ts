import { Context, Data, Effect, Layer, Stream } from "effect";
import { HttpClientRequest } from "effect/unstable/http";
import type { TemplateExpression } from "effect/unstable/process/ChildProcess";
import { ExitCode } from "effect/unstable/process/ChildProcessSpawner";

import { Client, type ClientError } from "./internal/client.ts";
import {
  endLines,
  execRequest,
  isTemplateStrings,
  renderShell,
  responseEvents,
  responseExec,
  responseResult,
  shellRequest,
  takeLines,
} from "./internal/process.ts";
import { route } from "./internal/prelude.ts";

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

export interface ProcessResult {
  /**
   * Waits for the child process to exit and returns the `ExitCode` of the
   * command that was run.
   */
  readonly exitCode: Effect.Effect<ExitCode, ProcessError>;

  /**
   * The standard output stream for the child process.
   */
  readonly stdout: Stream.Stream<Uint8Array, ProcessError>;
  /**
   * The standard error stream for the child process.
   */
  readonly stderr: Stream.Stream<Uint8Array, ProcessError>;
}

export interface CommandOptions {
  /**
   * The current working directory of the child process.
   */
  readonly cwd?: string | undefined;
  /**
   * The environment of the child process, layered over the inherited service
   * process environment and overriding the injected workspace variables.
   */
  readonly env?: Record<string, string | undefined> | undefined;
  /**
   * How long the server waits for the command to finish before answering with
   * the direct JSON result, in milliseconds.
   *
   * **Details**
   *
   * It bounds only the wait for a direct result and never terminates the
   * command. `Process.stream` defaults it to `0`, so the response upgrades as
   * soon as it can; `Process.result` and `Process.exec` default it to a positive
   * value so a fast command is answered directly.
   */
  readonly wait?: number | undefined;
}

export interface ShellCommandOptions extends CommandOptions {
  readonly shell?: string;
}

export type Command = Readonly<{
  command: string;
  args: ReadonlyArray<string>;
  options?: CommandOptions;
}>;

export class Process extends Context.Service<
  Process,
  {
    /**
     * Run a command and return its result, collecting the command's output
     * into a single value.
     *
     * **Details**
     *
     * Both response shapes are handled: the multiplexed frame stream is drained
     * when the server upgrades to it, and the direct result is used otherwise.
     */
    result(command: Command): Effect.Effect<ProcessResult, ProcessError>;

    /**
     * Run a command and stream its multiplexed output: the `Stdout` and
     * `Stderr` chunks as they arrive, ending with a single `Exit` event.
     *
     * **Details**
     *
     * Both response shapes are handled: the frame stream is decoded when the
     * server upgrades to it, and the events are synthesized from the direct
     * result otherwise. A command that could not run, or a stream that is
     * truncated or malformed, fails the stream.
     */
    stream(command: Command): Stream.Stream<ProcessEvent, ProcessError>;

    /**
     * Run a command and hand back whichever shape the server answered with:
     * the collected result for a direct response, the multiplexed event
     * stream for an upgraded one.
     */
    exec(
      command: Command,
    ): Effect.Effect<ProcessResult | Stream.Stream<ProcessEvent, ProcessError>, ProcessError>;

    /**
     * Run a shell script and return its standard output.
     *
     * **Details**
     *
     * Interpolated values are inlined verbatim. A command that exits with a
     * non-zero code fails with a `CommandExitError` carrying the script, its
     * exit code, and both output streams, rather than discarding them.
     */
    $: {
      (
        strings: TemplateStringsArray,
        ...values: ReadonlyArray<TemplateExpression>
      ): Effect.Effect<string, ProcessError>;
      (
        options: ShellCommandOptions,
      ): (
        strings: TemplateStringsArray,
        ...values: ReadonlyArray<TemplateExpression>
      ) => Effect.Effect<string, ProcessError>;
    };

    /**
     * Run a command and return its exit code.
     */
    exitCode(command: Command): Effect.Effect<ExitCode, ProcessError>;

    /**
     * Run a command and stream the lines of its output, without their line
     * endings.
     */
    lines(
      command: Command,
      options?: {
        readonly includeStderr?: boolean | undefined;
      },
    ): Stream.Stream<string, ProcessError>;

    /**
     * Run a command and return its output as a string.
     */
    string(
      command: Command,
      options?: {
        readonly includeStderr?: boolean | undefined;
      },
    ): Effect.Effect<string, ProcessError>;
  }
>()("process") {}

/**
 * The `wait` used by the operations that expect a direct result when the caller
 * does not pick one, so a fast command is answered with JSON instead of a stream.
 */
const DEFAULT_WAIT = 500;

export const make = Effect.fn("Process.make")(function* (
  options: { workspace?: string | undefined } = {},
) {
  const client = yield* Client;

  // The direct result needs a positive `wait`; without one the server streams
  // immediately, so the operations that expect a result choose this bound.
  const withWait = (command: Command, fallback: number): Command => ({
    ...command,
    options: { ...command.options, wait: command.options?.wait ?? fallback },
  });

  const execHttpRequest = (command: Command): HttpClientRequest.HttpClientRequest =>
    HttpClientRequest.post(route(options.workspace, "/exec")).pipe(
      HttpClientRequest.bodyJsonUnsafe(execRequest(command)),
    );

  const eventStream = (command: Command): Stream.Stream<ProcessEvent, ProcessError> =>
    Stream.unwrap(Effect.map(client.execute(execHttpRequest(command)), responseEvents));

  // The response shape is the server's choice: a command that outlives the probe
  // is answered with the frame stream, a shorter one with the direct result.
  const result = ((command) =>
    Effect.flatMap(
      client.execute(execHttpRequest(withWait(command, DEFAULT_WAIT))),
      responseResult,
    )) satisfies Process["Service"]["result"];

  const exec = ((command) =>
    Effect.flatMap(
      client.execute(execHttpRequest(withWait(command, DEFAULT_WAIT))),
      responseExec,
    )) satisfies Process["Service"]["exec"];

  const stream = ((command) =>
    eventStream(withWait(command, 0))) satisfies Process["Service"]["stream"];

  const runShell = (
    shellOptions: ShellCommandOptions,
    strings: TemplateStringsArray,
    values: ReadonlyArray<TemplateExpression>,
  ): Effect.Effect<string, ProcessError> =>
    Effect.gen(function* () {
      const script = renderShell(strings, values);

      const request = HttpClientRequest.post(route(options.workspace, "/exec")).pipe(
        HttpClientRequest.bodyJsonUnsafe(
          shellRequest(script, { ...shellOptions, wait: shellOptions.wait ?? DEFAULT_WAIT }),
        ),
      );

      const collected = yield* Effect.flatMap(client.execute(request), responseResult);
      const stdout = yield* Stream.mkString(Stream.decodeText(collected.stdout));
      const exitCode = yield* collected.exitCode;

      if (exitCode === 0) {
        return stdout;
      }

      const stderr = yield* Stream.mkString(Stream.decodeText(collected.stderr));

      return yield* Effect.fail(new CommandExitError({ script, exitCode, stdout, stderr }));
    });

  function $(
    strings: TemplateStringsArray,
    ...values: ReadonlyArray<TemplateExpression>
  ): Effect.Effect<string, ProcessError>;
  function $(
    shellOptions: ShellCommandOptions,
  ): (
    strings: TemplateStringsArray,
    ...values: ReadonlyArray<TemplateExpression>
  ) => Effect.Effect<string, ProcessError>;
  function $(
    first: TemplateStringsArray | ShellCommandOptions,
    ...values: ReadonlyArray<TemplateExpression>
  ):
    | Effect.Effect<string, ProcessError>
    | ((
        strings: TemplateStringsArray,
        ...values: ReadonlyArray<TemplateExpression>
      ) => Effect.Effect<string, ProcessError>) {
    if (isTemplateStrings(first)) {
      return runShell({}, first, values);
    }

    return (strings, ...args) => runShell(first, strings, args);
  }

  const outputText = (
    command: Command,
    outputOptions?: { readonly includeStderr?: boolean | undefined },
  ): Stream.Stream<string, ProcessError> =>
    Stream.unwrap(
      Effect.map(result(command), (collected) => {
        const stdout = Stream.decodeText(collected.stdout);

        return outputOptions?.includeStderr === true
          ? Stream.concat(stdout, Stream.decodeText(collected.stderr))
          : stdout;
      }),
    );

  const exitCode = ((command) =>
    Effect.flatMap(
      result(command),
      (collected) => collected.exitCode,
    )) satisfies Process["Service"]["exitCode"];

  const string = ((command, stringOptions) =>
    Stream.mkString(outputText(command, stringOptions))) satisfies Process["Service"]["string"];

  const lines = ((command, linesOptions) =>
    outputText(command, linesOptions).pipe(
      Stream.mapAccum(() => "", takeLines, { onHalt: endLines }),
    )) satisfies Process["Service"]["lines"];

  return Process.of({
    result,
    exec,
    $,
    stream,
    exitCode,
    lines,
    string,
  });
});

/**
 * The process service over a workspace, where `cwd` is a workspace-relative
 * path.
 */
export const layerForWorkspace = ({ workspace }: { workspace: string }) =>
  Layer.effect(Process, make({ workspace }));

/** The process service in direct mode, where `cwd` is an absolute path. */
export const layer = Layer.effect(Process, make());
