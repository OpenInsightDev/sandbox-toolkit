/**
 * An Effect-based client for the sandbox-toolkit HTTP server.
 *
 * ```ts
 * const program = Effect.gen(function* () {
 *   const client = yield* SandboxToolkit;
 *   return yield* client.readFile("/etc/hosts", { offset: 0, limit: 3 });
 * });
 *
 * Effect.runPromise(
 *   program.pipe(Effect.provide(SandboxToolkit.layer({ baseUrl: "http://127.0.0.1:8000" }))),
 * );
 * ```
 */

export type { Service as SandboxToolkitService } from "./SandboxToolkit.ts";
export { SandboxToolkit } from "./SandboxToolkit.ts";
export {
  InvalidRequestError,
  NotFoundError,
  SandboxToolkitError,
  ServerError,
  TransportError,
} from "./SandboxToolkitError.ts";
export { FileSystem } from "./FileSystem.ts";
export type { FileSystemService } from "./FileSystem.ts";
export { Process } from "./Process.ts";
export type { ProcessService } from "./Process.ts";
export { System } from "./System.ts";
export type { SystemService } from "./System.ts";
export { Tools } from "./Tools.ts";
export type { ToolsService } from "./Tools.ts";

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
  ShellParams,
  StatParams,
  StatResult,
  TextLine,
  WriteFileParams,
  WriteFileResult,
} from "./Schemas.ts";
