import { Context, Effect, Layer, Stream } from "effect";
import type { TemplateExpression } from "effect/unstable/process/ChildProcess";
import type { ExitCode } from "effect/unstable/process/ChildProcessSpawner";

export type ProcessError = never;

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
     * Spawn a command and return a handle for interaction.
     */
    spawn(command: Command): Effect.Effect<ProcessResult, ProcessError>;

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
     * Run a command and return the lines of its output as an array of strings.
     */
    lines(
      command: Command,
      options?: {
        readonly includeStderr?: boolean | undefined;
      },
    ): Effect.Effect<Array<string>, ProcessError>;

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

export const layerForWorkspace = ({ workspace }: { workspace: string }) =>
  Layer.effect(
    Process,
    Effect.gen(function* () {
      throw new Error("not implemented");
    }),
  );

export const layer = Layer.effect(
  Process,
  Effect.gen(function* () {
    throw new Error("not implemented");
  }),
);
