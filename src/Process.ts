import { Context, Effect, Layer, Stream } from "effect";
import { HttpClientRequest } from "effect/unstable/http";
import type { TemplateExpression } from "effect/unstable/process/ChildProcess";
import { ExitCode } from "effect/unstable/process/ChildProcessSpawner";

import type { ExecResult } from "./generated/ExecResult.ts";
import { Client } from "./internal/client.ts";
import {
  CommandFailed,
  ProcessEvent,
  StreamError,
  endLines,
  execRequest,
  isTemplateStrings,
  renderShell,
  responseEvents,
  responseExec,
  responseResult,
  shellRequest,
  takeLines,
  type ProcessError,
} from "./internal/process.ts";
import { route } from "./internal/prelude.ts";

export { CommandFailed, ProcessEvent, StreamError };

export type { ProcessError };

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
   * The environment of the child process.
   *
   * **Details**
   *
   * If `extendEnv` is set to `true`, the value of `env` will be merged with
   * the value of `globalThis.process.env`, prioritizing the values in `env`
   * when conflicts exist.
   *
   * **Gotchas**
   *
   * Without `extendEnv: true`, providing `env` replaces the inherited child
   * environment. The child will not receive `PATH` unless `env` includes it.
   */
  readonly env?: Record<string, string | undefined> | undefined;
  /**
   * How long the server waits for the command to finish before upgrading the
   * response to the frame stream, in milliseconds.
   *
   * **Details**
   *
   * It bounds only the wait for a direct result and never terminates the
   * command. `Process.stream` defaults it to `0`, so the response upgrades as
   * soon as it can; the other operations leave the server's default in place.
   */
  readonly timeout?: number | undefined;
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

export const make = Effect.fn("Process.make")(function* (
  options: { workspace?: string | undefined } = {},
) {
  const client = yield* Client;

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
      client.execute(execHttpRequest(command)),
      responseResult,
    )) satisfies Process["Service"]["result"];

  const exec = ((command) =>
    Effect.flatMap(
      client.execute(execHttpRequest(command)),
      responseExec,
    )) satisfies Process["Service"]["exec"];

  const stream = ((command) =>
    eventStream({
      ...command,
      options: { ...command.options, timeout: command.options?.timeout ?? 0 },
    })) satisfies Process["Service"]["stream"];

  const runShell = (
    shellOptions: ShellCommandOptions,
    strings: TemplateStringsArray,
    values: ReadonlyArray<TemplateExpression>,
  ): Effect.Effect<string, ProcessError> =>
    Effect.gen(function* () {
      const request = HttpClientRequest.post(route(options.workspace, "/shell")).pipe(
        HttpClientRequest.bodyJsonUnsafe(shellRequest(renderShell(strings, values), shellOptions)),
      );

      const result = yield* client.json<ExecResult>(request);

      return result.stdout;
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
