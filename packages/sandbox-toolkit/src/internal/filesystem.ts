import { ByteSize, Data, Effect, Match, Option, PlatformError, Predicate, Schema } from "effect";
import type { File, WatchEvent } from "effect/FileSystem";

import type { ResourceMetadata } from "../generated/ResourceMetadata.ts";
import { ApiError, type ClientError } from "./client.ts";

/** A `join` for the protocol's `/`-separated paths. */
export const joinPath = (from: string, name: string): string =>
  from === "" ? name : `${from.replace(/\/+$/, "")}/${name}`;

/** A `SystemError` reason without its tag, which the helper below supplies. */
type SystemFailure = Omit<Parameters<typeof PlatformError.systemError>[0], "_tag">;

/** Wraps a system failure as the platform error, keeping the tag in one place. */
const systemError = (
  tag: PlatformError.SystemErrorTag,
  failure: SystemFailure,
): PlatformError.PlatformError => PlatformError.systemError({ ...failure, _tag: tag });

/**
 * Operations whose server endpoint is not wired up yet fail rather than die,
 * so callers can recover.
 */
export const unsupported = (method: string): PlatformError.PlatformError =>
  systemError("Unknown", {
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
    const failure: SystemFailure = {
      module: "FileSystem",
      method,
      description: error.message,
      pathOrDescriptor: path,
      cause: error,
    };

    if (!(error instanceof ApiError)) {
      return systemError("Unknown", failure);
    }

    switch (error.code) {
      case "not_found":
        return systemError("NotFound", failure);
      case "not_a_file":
      case "not_a_directory":
        return systemError("BadResource", failure);
      case "conflict":
        return systemError(
          error.message.includes("already exists") ? "AlreadyExists" : "NotFound",
          failure,
        );
      case "read_only_workspace":
      case "managed_workspace":
        return systemError("PermissionDenied", failure);
      case "bad_request":
      case "invalid_request":
      case "unsupported_type":
      case "method_not_allowed":
        return PlatformError.badArgument({
          module: failure.module,
          method: failure.method,
          description: failure.description,
          cause: error,
        });
      default:
        return systemError("Unknown", failure);
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
  type: Match.value(metadata.kind).pipe(
    Match.when("file", () => "File" as const),
    Match.when("directory", () => "Directory" as const),
    Match.orElse(() => "SymbolicLink" as const),
  ),
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
  Predicate.isTagged(error.reason, "NotFound");

/** Wire shape of one NDJSON line of the `watch` stream. */
const watchEventSchema = Schema.fromJsonString(
  Schema.Struct({
    event: Schema.Literals(["create", "update", "remove"]),
    path: Schema.String,
  }),
);

/** Constructors for the file system watch event union, exposed by Effect as a type only. */
const fileWatchEvent = Data.taggedEnum<
  Data.TaggedEnum<{
    Create: { readonly path: string };
    Update: { readonly path: string };
    Remove: { readonly path: string };
  }>
>();

/** Maps a wire watch kind onto the file system event union. */
const toWatchEvent = (event: "create" | "update" | "remove", path: string): WatchEvent =>
  Match.value(event).pipe(
    Match.when("create", () => fileWatchEvent.Create({ path })),
    Match.when("update", () => fileWatchEvent.Update({ path })),
    Match.when("remove", () => fileWatchEvent.Remove({ path })),
    Match.exhaustive,
  );

/**
 * Decodes one NDJSON line of the watch stream. A line that does not decode is a
 * protocol violation, so it fails the stream instead of being dropped.
 */
export const parseWatchEvent = (
  line: string,
): Effect.Effect<WatchEvent, PlatformError.PlatformError> =>
  Option.match(Schema.decodeUnknownOption(watchEventSchema)(line), {
    onNone: () =>
      Effect.fail(
        systemError("Unknown", {
          module: "FileSystem",
          method: "watch",
          description: `malformed watch event: ${line}`,
        }),
      ),
    onSome: (event) => Effect.succeed(toWatchEvent(event.event, event.path)),
  });
