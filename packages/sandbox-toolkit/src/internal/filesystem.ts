import { ByteSize, Data, Effect, Match, Option, PlatformError, Predicate, Schema } from "effect";
import type { File, WatchEvent } from "effect/FileSystem";

import type { Stat } from "../generated/Stat.ts";
import { ApiError, type ClientError } from "./client.ts";
import type { UploadError } from "./TUSClient.ts";

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

/** The platform error a response status classifies to; an absent status is unknown. */
const statusError = (
  status: number | undefined,
  failure: SystemFailure,
): PlatformError.PlatformError => {
  switch (status) {
    case 400:
      return PlatformError.badArgument({
        module: failure.module,
        method: failure.method,
        description: failure.description,
        cause: failure.cause,
      });
    case 403:
      return systemError("PermissionDenied", failure);
    case 404:
      return systemError("NotFound", failure);
    case 409:
      return systemError("AlreadyExists", failure);
    case 422:
      return systemError("BadResource", failure);
    default:
      return systemError("Unknown", failure);
  }
};

/**
 * The mount answers a status and a human-readable brief rather than a coded error
 * document, so the status carries the classification.
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

    return statusError(error instanceof ApiError ? error.status : undefined, failure);
  };

/**
 * The upload client reports failures of its own, carrying the status the mount
 * answered with, so they classify through the same table as a mount request.
 */
export const toPlatformUploadError =
  (method: string, path: string) =>
  (error: UploadError): PlatformError.PlatformError => {
    const failure: SystemFailure = {
      module: "FileSystem",
      method,
      description: error.message,
      pathOrDescriptor: path,
      cause: error,
    };

    return statusError(error.status, failure);
  };

/** A server timestamp as a `Date`, absent when the platform withheld it. */
const optionalDate = (value: string | null): Option.Option<Date> =>
  value === null ? Option.none() : Option.some(new Date(value));

/**
 * Maps the server's metadata onto `File.Info`. Fields the platform withheld stay
 * empty rather than fabricated.
 */
export const metadataInfo = (metadata: Stat): File.Info => ({
  type: Match.value(metadata.kind).pipe(
    Match.when("file", () => "File" as const),
    Match.when("directory", () => "Directory" as const),
    Match.orElse(() => "SymbolicLink" as const),
  ),
  mtime: Option.some(new Date(metadata.modified_at)),
  atime: optionalDate(metadata.accessed_at),
  birthtime: optionalDate(metadata.birthtime),
  dev: metadata.device,
  ino: Option.some(metadata.inode),
  mode: Number.parseInt(metadata.mode, 8),
  nlink: Option.some(metadata.links),
  uid: Option.some(metadata.uid),
  gid: Option.some(metadata.gid),
  rdev: Option.some(metadata.device_type),
  size: ByteSize.bytes(metadata.size),
  blksize: Option.some(ByteSize.bytes(metadata.block_size)),
  blocks: Option.some(metadata.blocks),
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
