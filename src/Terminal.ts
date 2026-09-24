import {
  Context,
  type Cause,
  type Effect,
  type PlatformError,
  type Queue,
  type Scope,
} from "effect";
import type { QuitError, UserInput } from "effect/Terminal";

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
