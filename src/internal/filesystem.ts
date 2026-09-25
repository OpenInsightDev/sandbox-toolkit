import { ByteSize, Effect, Option, PlatformError } from "effect";
import type { File } from "effect/FileSystem";

import type { ResourceMetadata } from "../generated/ResourceMetadata.ts";
import { ApiError, type ClientError } from "./client.ts";

/** Percent-encode each segment, keeping the `/` separators. */
const encodePath = (path: string): string => path.split("/").map(encodeURIComponent).join("/");

/**
 * The resource URL `path` addresses. Workspace mode keeps the path relative to
 * the workspace prefix, where "" addresses the root; direct mode spells an
 * absolute path after `/fs`, which a relative path cannot express, so it is
 * rejected before the request rather than fixed up.
 */
export const resourceUrl = (
  workspace: string | undefined,
  method: string,
  path: string,
): Effect.Effect<string, PlatformError.PlatformError> => {
  if (workspace !== undefined) {
    return Effect.succeed(
      `/workspaces/${workspace}/fs${path === "" ? "" : `/${encodePath(path)}`}`,
    );
  }

  return path.startsWith("/")
    ? Effect.succeed(`/fs${encodePath(path)}`)
    : Effect.fail(
        PlatformError.badArgument({
          module: "FileSystem",
          method,
          description: `path must be absolute: ${path}`,
        }),
      );
};

/** A `join` for the protocol's `/`-separated paths. */
export const joinPath = (from: string, name: string): string =>
  from === "" ? name : `${from.replace(/\/+$/, "")}/${name}`;

/**
 * Operations whose server endpoint is not wired up yet fail rather than die,
 * so callers can recover.
 */
export const unsupported = (method: string): PlatformError.PlatformError =>
  PlatformError.systemError({
    _tag: "Unknown",
    module: "FileSystem",
    method,
    description: "the sandbox file server does not implement this operation yet",
  });

/**
 * Maps the shared error envelope onto the platform error model. The `conflict`
 * code covers both an occupied target and a missing parent, which only the
 * message tells apart.
 */
export const toPlatformError =
  (method: string, path: string) =>
  (error: ClientError): PlatformError.PlatformError => {
    if (!(error instanceof ApiError)) {
      return PlatformError.systemError({
        _tag: "Unknown",
        module: "FileSystem",
        method,
        description: error.message,
        pathOrDescriptor: path,
        cause: error,
      });
    }

    const options = {
      module: "FileSystem",
      method,
      description: error.message,
      pathOrDescriptor: path,
      cause: error,
    } as const;

    switch (error.code) {
      case "not_found":
        return PlatformError.systemError({ ...options, _tag: "NotFound" });
      case "not_a_file":
      case "not_a_directory":
        return PlatformError.systemError({ ...options, _tag: "BadResource" });
      case "conflict":
        return PlatformError.systemError({
          ...options,
          _tag: error.message.includes("already exists") ? "AlreadyExists" : "NotFound",
        });
      case "read_only_workspace":
      case "managed_workspace":
        return PlatformError.systemError({ ...options, _tag: "PermissionDenied" });
      case "bad_request":
      case "invalid_request":
      case "unsupported_type":
      case "method_not_allowed":
        return PlatformError.badArgument({
          module: options.module,
          method: options.method,
          description: options.description,
          cause: error,
        });
      default:
        return PlatformError.systemError({ ...options, _tag: "Unknown" });
    }
  };

/** A server timestamp as a `Date`, absent when the platform withheld it. */
const optionalDate = (value: string | undefined): Option.Option<Date> =>
  value === undefined ? Option.none() : Option.some(new Date(value));

/** Permission bits as a number parsed from the octal string, `0` when withheld. */
const permissionBits = (mode: string | undefined): number =>
  mode === undefined ? 0 : Number.parseInt(mode, 8);

/** Block size as `ByteSize`, absent when the platform withheld it. */
const optionalByteSize = (value: number | undefined): Option.Option<ByteSize.ByteSize> =>
  value === undefined ? Option.none() : Option.some(ByteSize.bytes(value));

/**
 * Map the server's metadata onto `File.Info`. Fields the platform withheld stay
 * empty rather than fabricated.
 */
export const metadataInfo = (metadata: ResourceMetadata): File.Info => ({
  type:
    metadata.kind === "file"
      ? "File"
      : metadata.kind === "directory"
        ? "Directory"
        : "SymbolicLink",
  mtime: Option.some(new Date(metadata.modified_at)),
  atime: optionalDate(metadata.accessed_at),
  birthtime: optionalDate(metadata.birthtime),
  dev: metadata.device ?? 0,
  ino: Option.fromNullishOr(metadata.inode),
  mode: permissionBits(metadata.mode),
  nlink: Option.fromNullishOr(metadata.links),
  uid: Option.fromNullishOr(metadata.uid),
  gid: Option.fromNullishOr(metadata.gid),
  rdev: Option.fromNullishOr(metadata.device_type),
  size: ByteSize.bytes(metadata.size),
  blksize: optionalByteSize(metadata.block_size),
  blocks: Option.fromNullishOr(metadata.blocks),
});

/** The resource reports as absent only when the server said `not_found`. */
export const isNotFound = (error: PlatformError.PlatformError): boolean =>
  error.reason._tag === "NotFound";
