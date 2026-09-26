import {
  Context,
  Effect,
  Layer,
  PlatformError,
  Sink,
  Stream,
  type ByteSize,
  type Scope,
} from "effect";
import type { OpenFlag, File, WatchOptions, WatchEvent } from "effect/FileSystem";
import { HttpClientRequest } from "effect/unstable/http";

import type { CreateDirectoryRequest } from "./generated/CreateDirectoryRequest.ts";
import type { DirectoryResponse } from "./generated/DirectoryResponse.ts";
import type { GlobRequest } from "./generated/GlobRequest.ts";
import type { ListRequest } from "./generated/ListRequest.ts";
import type { ResourceMetadata } from "./generated/ResourceMetadata.ts";
import { Client } from "./internal/client.ts";
import { isNotFound, metadataInfo, toPlatformError, unsupported } from "./internal/filesystem.ts";
import { endLines, takeLines } from "./internal/process.ts";

export type FileSystemError = PlatformError.PlatformError;

/**
 * One filesystem request: the `operation` names the caller in errors, `type`
 * selects the wire handler, and `body` carries `path` (plus any operation
 * parameters) in JSON rather than in the URL.
 */
interface Query<Body extends { readonly path: string }> {
  readonly operation: string;
  readonly type: string;
  readonly body: Body;
}

export interface FileSystem {
  readonly access: (
    path: string,
    options?: {
      readonly ok?: boolean | undefined;
      readonly readable?: boolean | undefined;
      readonly writable?: boolean | undefined;
    },
  ) => Effect.Effect<void, FileSystemError>;

  readonly copy: (
    fromPath: string,
    toPath: string,
    options?: {
      readonly overwrite?: boolean | undefined;
      readonly preserveTimestamps?: boolean | undefined;
    },
  ) => Effect.Effect<void, FileSystemError>;

  readonly copyFile: (fromPath: string, toPath: string) => Effect.Effect<void, FileSystemError>;

  readonly chmod: (path: string, mode: number) => Effect.Effect<void, FileSystemError>;

  readonly glob: (
    pattern: string,
    options?: {
      readonly root?: string | undefined;
      readonly exclude?: ReadonlyArray<string> | undefined;
    },
  ) => Effect.Effect<Array<string>, FileSystemError>;

  readonly exists: (path: string) => Effect.Effect<boolean, FileSystemError>;

  readonly symlink: (fromPath: string, toPath: string) => Effect.Effect<void, FileSystemError>;

  readonly makeDirectory: (
    path: string,
    options?: {
      readonly recursive?: boolean | undefined;
    },
  ) => Effect.Effect<void, FileSystemError>;

  readonly makeTempDirectory: (options?: {
    readonly directory?: string | undefined;
    readonly prefix?: string | undefined;
  }) => Effect.Effect<string, FileSystemError>;

  readonly makeTempDirectoryScoped: (options?: {
    readonly directory?: string | undefined;
    readonly prefix?: string | undefined;
  }) => Effect.Effect<string, FileSystemError, Scope.Scope>;

  readonly makeTempFile: (options?: {
    readonly directory?: string | undefined;
    readonly prefix?: string | undefined;
    readonly suffix?: string | undefined;
  }) => Effect.Effect<string, FileSystemError>;

  readonly makeTempFileScoped: (options?: {
    readonly directory?: string | undefined;
    readonly prefix?: string | undefined;
    readonly suffix?: string | undefined;
  }) => Effect.Effect<string, FileSystemError, Scope.Scope>;

  readonly readDirectory: (
    path: string,
    options?: {
      readonly recursive?: boolean | undefined;
    },
  ) => Effect.Effect<Array<string>, FileSystemError>;

  readonly readFile: (path: string) => Effect.Effect<Uint8Array, FileSystemError>;

  readonly readLines: (path: string) => Stream.Stream<string, FileSystemError>;

  readonly readFileString: (
    path: string,
    encoding?: string,
  ) => Effect.Effect<string, FileSystemError>;

  readonly readLink: (path: string) => Effect.Effect<string, FileSystemError>;

  readonly realPath: (path: string) => Effect.Effect<string, FileSystemError>;

  readonly remove: (
    path: string,
    options?: {
      /**
       * When `true`, you can recursively remove nested directories.
       */
      readonly recursive?: boolean | undefined;
      /**
       * When `true`, exceptions will be ignored if `path` does not exist.
       */
      readonly force?: boolean | undefined;
    },
  ) => Effect.Effect<void, FileSystemError>;

  readonly rename: (oldPath: string, newPath: string) => Effect.Effect<void, FileSystemError>;

  readonly sink: (
    path: string,
    options?: {
      readonly flag?: OpenFlag | undefined;
      readonly mode?: number | undefined;
    },
  ) => Sink.Sink<void, Uint8Array, never, FileSystemError>;

  readonly stat: (path: string) => Effect.Effect<File.Info, FileSystemError>;

  readonly stream: (
    path: string,
    options?: {
      readonly bytesToRead?: ByteSize.Input | undefined;
      readonly chunkSize?: number | undefined;
      readonly offset?: ByteSize.Input | undefined;
    },
  ) => Stream.Stream<Uint8Array, FileSystemError>;

  readonly truncate: (path: string, length?: number) => Effect.Effect<void, FileSystemError>;

  readonly utimes: (
    path: string,
    atime: Date | number,
    mtime: Date | number,
  ) => Effect.Effect<void, FileSystemError>;

  readonly watch: (
    path: string,
    options?: WatchOptions,
  ) => Stream.Stream<WatchEvent, FileSystemError>;

  readonly writeFile: (
    path: string,
    data: Uint8Array,
    options?: {
      readonly flag?: OpenFlag | undefined;
      readonly mode?: number | undefined;
    },
  ) => Effect.Effect<void, FileSystemError>;

  readonly writeFileString: (
    path: string,
    data: string,
    options?: {
      readonly flag?: OpenFlag | undefined;
      readonly mode?: number | undefined;
    },
  ) => Effect.Effect<void, FileSystemError>;
}

export const FileSystem: Context.Service<FileSystem, FileSystem> =
  Context.Service("effect/FileSystem");

export const make = Effect.fn("FileSystem.make")(function* (
  options: { workspace?: string | undefined } = {},
) {
  const client = yield* Client;

  const endpoint = (type: string): string =>
    options.workspace === undefined
      ? `/fs?type=${type}`
      : `/workspaces/${options.workspace}/fs?type=${type}`;

  // Direct mode addresses absolute paths only, so a relative one is rejected
  // before the request rather than fixed up.
  const guardPath = ({
    operation,
    body,
  }: Query<{ readonly path: string }>): Effect.Effect<void, FileSystemError> =>
    options.workspace === undefined && !body.path.startsWith("/")
      ? Effect.fail(
          PlatformError.badArgument({
            module: "FileSystem",
            method: operation,
            description: `path must be absolute: ${body.path}`,
          }),
        )
      : Effect.void;

  // The path travels in the JSON body; only the wire `type` is in the query.
  const wire = <Body extends { readonly path: string }>({
    type,
    body,
  }: Query<Body>): HttpClientRequest.HttpClientRequest =>
    HttpClientRequest.query(endpoint(type)).pipe(HttpClientRequest.bodyJsonUnsafe(body));

  const queryJson = <A, Body extends { readonly path: string } = { readonly path: string }>(
    query: Query<Body>,
  ): Effect.Effect<A, FileSystemError> =>
    guardPath(query).pipe(
      Effect.flatMap(() =>
        client
          .json<A>(wire(query))
          .pipe(Effect.mapError(toPlatformError(query.operation, query.body.path))),
      ),
    );

  const queryBytes = (
    query: Query<{ readonly path: string }>,
  ): Effect.Effect<Uint8Array, FileSystemError> =>
    guardPath(query).pipe(
      Effect.flatMap(() =>
        client
          .bytes(wire(query))
          .pipe(Effect.mapError(toPlatformError(query.operation, query.body.path))),
      ),
    );

  const queryStream = (
    query: Query<{ readonly path: string }>,
  ): Stream.Stream<Uint8Array, FileSystemError> =>
    Stream.unwrap(
      guardPath(query).pipe(
        Effect.map(() =>
          client
            .stream(wire(query))
            .pipe(Stream.mapError(toPlatformError(query.operation, query.body.path))),
        ),
      ),
    );

  const readFile = ((path: string) =>
    queryBytes({
      operation: "readFile",
      type: "stream",
      body: { path },
    })) satisfies FileSystem["readFile"];

  const stream = ((path: string, streamOptions) => {
    if (streamOptions?.offset !== undefined || streamOptions?.bytesToRead !== undefined) {
      return Stream.fail(unsupported("stream(offset/bytesToRead)"));
    }

    return queryStream({ operation: "stream", type: "stream", body: { path } });
  }) satisfies FileSystem["stream"];

  const stat = ((path: string) =>
    queryJson<ResourceMetadata>({ operation: "stat", type: "metadata", body: { path } }).pipe(
      Effect.map(metadataInfo),
    )) satisfies FileSystem["stat"];

  const exists = ((path: string) =>
    stat(path).pipe(
      Effect.as(true),
      Effect.catchIf(isNotFound, () => Effect.succeed(false)),
    )) satisfies FileSystem["exists"];

  const readDirectory = ((path: string, readOptions) => {
    const body: ListRequest = {
      path,
      offset: 0,
      limit: null,
      depth: readOptions?.recursive === true ? "infinity" : null,
    };

    // Entries carry the addressing-mode path; rebuilding from the name would drop
    // the directory prefix of a recursive listing.
    return queryJson<DirectoryResponse>({ operation: "readDirectory", type: "list", body }).pipe(
      Effect.map((directory) => directory.entries.map((entry) => entry.path)),
    );
  }) satisfies FileSystem["readDirectory"];

  const glob = ((pattern: string, globOptions) => {
    const body: GlobRequest = {
      path: globOptions?.root ?? "",
      pattern,
      exclude: [...(globOptions?.exclude ?? [])],
      offset: 0,
      limit: null,
    };

    // Matched entries carry the addressing-mode path, so they round-trip in
    // both modes.
    return queryJson<DirectoryResponse>({ operation: "glob", type: "glob", body }).pipe(
      Effect.map((directory) => directory.entries.map((entry) => entry.path)),
    );
  }) satisfies FileSystem["glob"];

  const makeDirectory = ((path: string, makeOptions) => {
    const body: CreateDirectoryRequest = {
      path,
      recursive: makeOptions?.recursive ?? null,
    };

    const query = { operation: "makeDirectory", type: "directory", body } as const;

    return guardPath(query).pipe(
      Effect.flatMap(() =>
        client
          .void(
            HttpClientRequest.put(endpoint(query.type)).pipe(
              HttpClientRequest.setHeader("if-none-match", "*"),
              HttpClientRequest.bodyJsonUnsafe(query.body),
            ),
          )
          .pipe(Effect.mapError(toPlatformError(query.operation, query.body.path))),
      ),
    );
  }) satisfies FileSystem["makeDirectory"];

  const readFileString = ((path: string, encoding?: string) =>
    readFile(path).pipe(
      Effect.map((bytes) => new TextDecoder(encoding ?? "utf-8").decode(bytes)),
    )) satisfies FileSystem["readFileString"];

  const readLines = ((path: string) =>
    stream(path, {}).pipe(
      Stream.decodeText,
      Stream.mapAccum(() => "", takeLines, { onHalt: endLines }),
    )) satisfies FileSystem["readLines"];

  const fails = (method: string): Effect.Effect<never, FileSystemError> =>
    Effect.fail(unsupported(method));

  return FileSystem.of({
    access: () => fails("access"),
    copy: () => fails("copy"),
    copyFile: () => fails("copyFile"),
    chmod: () => fails("chmod"),
    glob,
    exists,
    symlink: () => fails("symlink"),
    makeDirectory,
    makeTempDirectory: () => fails("makeTempDirectory"),
    makeTempDirectoryScoped: () => fails("makeTempDirectoryScoped"),
    makeTempFile: () => fails("makeTempFile"),
    makeTempFileScoped: () => fails("makeTempFileScoped"),
    readDirectory,
    readFile,
    readLines,
    readFileString,
    readLink: () => fails("readLink"),
    realPath: () => fails("realPath"),
    remove: () => fails("remove"),
    rename: () => fails("rename"),
    sink: () => Sink.fail(unsupported("sink")),
    stat,
    stream,
    truncate: () => fails("truncate"),
    utimes: () => fails("utimes"),
    watch: () => Stream.fail(unsupported("watch")),
    writeFile: () => fails("writeFile"),
    writeFileString: () => fails("writeFileString"),
  });
});

/**
 * The file system service over a workspace, where paths are workspace-relative
 * and "" addresses the workspace root.
 */
export const layerForWorkspace = ({ workspace }: { workspace: string }) =>
  Layer.effect(FileSystem, make({ workspace }));

/** The file system service in direct mode, where paths are absolute. */
export const layer = Layer.effect(FileSystem, make());
