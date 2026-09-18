/**
 * An Effect-based client for the sandbox-toolkit HTTP server.
 *
 * ```ts
 * const program = Effect.gen(function* () {
 *   const client = yield* SandboxToolkit;
 *   return yield* client.readFile({ path: "/etc/hosts", offset: 0, limit: 3 });
 * });
 *
 * Effect.runPromise(
 *   program.pipe(Effect.provide(SandboxToolkit.layer({ baseUrl: "http://127.0.0.1:8000" }))),
 * );
 * ```
 */

export type { Service as SandboxToolkitService } from "./client.ts";
export { SandboxToolkit } from "./client.ts";
export {
  InvalidRequestError,
  NotFoundError,
  SandboxToolkitError,
  ServerError,
  TransportError,
} from "./errors.ts";

export type {
  CopyParams,
  CopyResult,
  DescribeToolParams,
  DescribeToolResult,
  ExecParams,
  ExecResult,
  HealthResult,
  ListParams,
  ListResult,
  ListToolsResult,
  MkdirParams,
  MkdirResult,
  MoveParams,
  MoveResult,
  ReadFileParams,
  ReadFileResult,
  RemoveParams,
  RemoveResult,
  Resource,
  ResourceKind,
  StatParams,
  StatResult,
  TextLine,
  WriteFileParams,
  WriteFileResult,
} from "./schemas.ts";
