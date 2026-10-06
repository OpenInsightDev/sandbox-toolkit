import {
  Context,
  Effect,
  Layer,
  Option,
  PlatformError,
  Sink,
  Stream,
  type ByteSize,
  type Scope,
} from "effect";
import type { OpenFlag, File, WatchOptions, WatchEvent } from "effect/FileSystem";
import { HttpClientRequest } from "effect/unstable/http";

import type { AccessRequest } from "./generated/AccessRequest.ts";
import type { CommitRequest } from "./generated/CommitRequest.ts";
import type { Content } from "./generated/Content.ts";
import type { Entries } from "./generated/Entries.ts";
import type { GlobRequest } from "./generated/GlobRequest.ts";
import type { Lines } from "./generated/Lines.ts";
import type { LinesRequest } from "./generated/LinesRequest.ts";
import type { ListRequest } from "./generated/ListRequest.ts";
import type { MakeDirectoryRequest } from "./generated/MakeDirectoryRequest.ts";
import type { MetadataRequest } from "./generated/MetadataRequest.ts";
import type { RealPath } from "./generated/RealPath.ts";
import type { RemoveRequest } from "./generated/RemoveRequest.ts";
import type { Stat } from "./generated/Stat.ts";
import type { SymlinkRequest } from "./generated/SymlinkRequest.ts";
import type { TransferRequest } from "./generated/TransferRequest.ts";
import type { TruncateRequest } from "./generated/TruncateRequest.ts";
import type { WatchRequest } from "./generated/WatchRequest.ts";
import type { WriteRequest } from "./generated/WriteRequest.ts";
import { Client, layer as clientLayer } from "./internal/client.ts";
import {
  isNotFound,
  metadataInfo,
  parseWatchEvent,
  toPlatformError,
  toPlatformUploadError,
  unsupported,
} from "./internal/filesystem.ts";
import { endLines, takeLines } from "./internal/process.ts";
import { TUSClient, layer as tusLayer } from "./internal/TUSClient.ts";

export type FileSystemError = PlatformError.PlatformError;

/** The HTTP methods the mount accepts; each names one handler group. */
type Method = "query" | "put" | "patch" | "post" | "delete";

const methods: Readonly<Record<Method, (url: string) => HttpClientRequest.HttpClientRequest>> = {
  query: HttpClientRequest.query,
  put: HttpClientRequest.put,
  patch: HttpClientRequest.patch,
  post: HttpClientRequest.post,
  delete: HttpClientRequest.delete,
};

/** Every request body names a path; a transfer names its destination too. */
interface Addressing {
  readonly path: string;
  readonly destination?: string | undefined;
}

/**
 * One file system request: `operation` names the caller in errors, `method` and
 * `type` select the handler, and `body` carries the document.
 */
interface Query<Body extends Addressing> {
  readonly operation: string;
  /** The mount's read method unless a write names its own. */
  readonly method?: Method | undefined;
  readonly type: string | undefined;
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

const isUtf8 = (encoding: string): boolean => encoding.replaceAll("-", "").toLowerCase() === "utf8";

/** The mount's timestamp patch takes RFC 3339. */
const timestamp = (value: Date | number): string => new Date(value).toISOString();

/**
 * The `PATCH` body size a streamed sink uploads in. tus requires a finite size
 * for a stream source, and it bounds how much of the sink's input is in flight.
 */
const sinkChunkSize = 1024 * 1024;

/** The upload id a tus URL ends in, which `commit` addresses the staged bytes by. */
const uploadId = (url: string): string => url.slice(url.lastIndexOf("/") + 1);

export const make = Effect.fn("FileSystem.make")(function* (
  options: { workspace?: string | undefined } = {},
) {
  const client = yield* Client;
  const tus = yield* TUSClient;

  // `DELETE` names its operation by method alone, so it carries no `type`.
  const endpoint = (type: string | undefined): string => {
    const mount = options.workspace === undefined ? "/fs" : `/workspaces/${options.workspace}/fs`;

    return type === undefined ? mount : `${mount}?type=${type}`;
  };

  // The direct mount addresses absolute paths, so a relative one is rejected
  // before the request rather than resolved against the server's own cwd.
  const guardPaths = (
    operation: string,
    body: Addressing,
  ): Effect.Effect<void, FileSystemError> => {
    if (options.workspace !== undefined) {
      return Effect.void;
    }

    const paths = body.destination === undefined ? [body.path] : [body.path, body.destination];
    const relative = paths.find((path) => !path.startsWith("/"));

    return relative === undefined
      ? Effect.void
      : Effect.fail(
          PlatformError.badArgument({
            module: "FileSystem",
            method: operation,
            description: `path must be absolute: ${relative}`,
          }),
        );
  };

  const wire = <Body extends Addressing>({
    method = "query",
    type,
    body,
  }: Query<Body>): HttpClientRequest.HttpClientRequest =>
    methods[method](endpoint(type)).pipe(HttpClientRequest.bodyJsonUnsafe(body));

  const queryJson = <A, Body extends Addressing = Addressing>(
    query: Query<Body>,
  ): Effect.Effect<A, FileSystemError> =>
    guardPaths(query.operation, query.body).pipe(
      Effect.flatMap(() =>
        client
          .json<A>(wire(query))
          .pipe(Effect.mapError(toPlatformError(query.operation, query.body.path))),
      ),
    );

  const queryBytes = <Body extends Addressing>(
    query: Query<Body>,
  ): Effect.Effect<Uint8Array, FileSystemError> =>
    guardPaths(query.operation, query.body).pipe(
      Effect.flatMap(() =>
        client
          .bytes(wire(query))
          .pipe(Effect.mapError(toPlatformError(query.operation, query.body.path))),
      ),
    );

  const queryVoid = <Body extends Addressing>(
    query: Query<Body>,
  ): Effect.Effect<void, FileSystemError> =>
    guardPaths(query.operation, query.body).pipe(
      Effect.flatMap(() =>
        client
          .void(wire(query))
          .pipe(Effect.mapError(toPlatformError(query.operation, query.body.path))),
      ),
    );

  const queryStream = <Body extends Addressing>(
    query: Query<Body>,
  ): Stream.Stream<Uint8Array, FileSystemError> =>
    Stream.unwrap(
      guardPaths(query.operation, query.body).pipe(
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

  // A read in the decoder's own encoding is the mount's `content` read, which
  // answers UTF-8 or rejects the content; another encoding reads the raw bytes.
  const readFileString = ((path: string, encoding?: string) =>
    encoding === undefined || isUtf8(encoding)
      ? queryJson<Content>({
          operation: "readFileString",
          type: "content",
          body: { path },
        }).pipe(Effect.map((body) => body.content))
      : readFile(path).pipe(
          Effect.map((bytes) => new TextDecoder(encoding).decode(bytes)),
        )) satisfies FileSystem["readFileString"];

  const stat = ((path: string) =>
    queryJson<Stat>({ operation: "stat", type: "metadata", body: { path } }).pipe(
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
      depth: readOptions?.recursive === true ? "infinity" : null,
      offset: 0,
      limit: null,
    };

    // Entries carry the addressing-mode path; rebuilding from the name would drop
    // the directory prefix of a recursive listing.
    return queryJson<Entries>({ operation: "readDirectory", type: "list", body }).pipe(
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
    return queryJson<Entries>({ operation: "glob", type: "glob", body }).pipe(
      Effect.map((directory) => directory.entries.map((entry) => entry.path)),
    );
  }) satisfies FileSystem["glob"];

  const access = ((path: string, accessOptions) => {
    const body: AccessRequest = {
      path,
      ok: accessOptions?.ok ?? null,
      readable: accessOptions?.readable ?? null,
      writable: accessOptions?.writable ?? null,
    };

    return queryVoid({ operation: "access", type: "access", body });
  }) satisfies FileSystem["access"];

  const readLines = ((path: string) =>
    Stream.paginate(0, (offset: number) =>
      queryJson<Lines, LinesRequest>({
        operation: "readLines",
        type: "lines",
        body: { path, offset, limit: null },
      }).pipe(
        Effect.map(
          (page) =>
            [
              page.lines,
              page.truncated && page.lines.length > 0
                ? Option.some(offset + page.lines.length)
                : Option.none(),
            ] as const,
        ),
      ),
    )) satisfies FileSystem["readLines"];

  const realPath = ((path: string) =>
    queryJson<RealPath>({ operation: "realPath", type: "realpath", body: { path } }).pipe(
      Effect.map((resolved) => resolved.path),
    )) satisfies FileSystem["realPath"];

  const watch = ((path: string, watchOptions) => {
    const body: WatchRequest = {
      path,
      recursive: watchOptions?.recursive ?? null,
    };

    // The server pushes one JSON event per line for as long as the response
    // stays open; ending the stream closes the connection and the watcher.
    return queryStream({ operation: "watch", type: "watch", body }).pipe(
      Stream.decodeText,
      Stream.mapAccum(() => "", takeLines, { onHalt: endLines }),
      Stream.mapEffect(parseWatchEvent),
    );
  }) satisfies FileSystem["watch"];

  const transfer = (
    operation: string,
    type: string,
    path: string,
    destination: string,
  ): Effect.Effect<void, FileSystemError> =>
    queryVoid({
      operation,
      method: "post",
      type,
      body: { path, destination } satisfies TransferRequest,
    });

  const copy = ((fromPath: string, toPath: string, copyOptions) => {
    if (copyOptions?.overwrite === false) {
      return Effect.fail(unsupported("copy(overwrite=false)"));
    }

    if (copyOptions?.preserveTimestamps === true) {
      return Effect.fail(unsupported("copy(preserveTimestamps)"));
    }

    return transfer("copy", "copy", fromPath, toPath);
  }) satisfies FileSystem["copy"];

  const copyFile = ((fromPath: string, toPath: string) =>
    transfer("copyFile", "copy", fromPath, toPath)) satisfies FileSystem["copyFile"];

  const rename = ((oldPath: string, newPath: string) =>
    transfer("rename", "move", oldPath, newPath)) satisfies FileSystem["rename"];

  // The mount's metadata patch writes only the fields it is given, so an
  // untouched attribute is sent as `null`.
  const chmod = ((path: string, mode: number) =>
    queryVoid({
      operation: "chmod",
      method: "patch",
      type: "metadata",
      body: {
        path,
        mode: mode.toString(8),
        uid: null,
        gid: null,
        atime: null,
        mtime: null,
      } satisfies MetadataRequest,
    })) satisfies FileSystem["chmod"];

  const utimes = ((path: string, atime: Date | number, mtime: Date | number) =>
    queryVoid({
      operation: "utimes",
      method: "patch",
      type: "metadata",
      body: {
        path,
        mode: null,
        uid: null,
        gid: null,
        atime: timestamp(atime),
        mtime: timestamp(mtime),
      } satisfies MetadataRequest,
    })) satisfies FileSystem["utimes"];

  const truncate = ((path: string, length?: number) =>
    queryVoid({
      operation: "truncate",
      method: "patch",
      type: "truncate",
      body: { path, length: length ?? null } satisfies TruncateRequest,
    })) satisfies FileSystem["truncate"];

  const symlink = ((fromPath: string, toPath: string) =>
    queryVoid({
      operation: "symlink",
      method: "put",
      type: "symlink",
      // The link is created at `toPath` and points at `fromPath`.
      body: { path: toPath, target: fromPath } satisfies SymlinkRequest,
    })) satisfies FileSystem["symlink"];

  const makeDirectory = ((path: string, makeOptions) =>
    queryVoid({
      operation: "makeDirectory",
      method: "put",
      type: "directory",
      body: { path, recursive: makeOptions?.recursive ?? null } satisfies MakeDirectoryRequest,
    })) satisfies FileSystem["makeDirectory"];

  const remove = ((path: string, removeOptions) =>
    queryVoid({
      operation: "remove",
      method: "delete",
      type: undefined,
      body: {
        path,
        recursive: removeOptions?.recursive ?? null,
        force: removeOptions?.force ?? null,
      } satisfies RemoveRequest,
    })) satisfies FileSystem["remove"];

  // The mount's file write is `open("w")`: it always creates or truncates and
  // leaves the mode to the process umask, so any other flag or an explicit mode
  // is a request the endpoint cannot honor.
  const guardWrite = (
    method: string,
    writeOptions?: { readonly flag?: OpenFlag | undefined; readonly mode?: number | undefined },
  ): Effect.Effect<void, FileSystemError> => {
    if (writeOptions?.flag !== undefined && writeOptions.flag !== "w") {
      return Effect.fail(unsupported(`${method}(flag)`));
    }

    if (writeOptions?.mode !== undefined) {
      return Effect.fail(unsupported(`${method}(mode)`));
    }

    return Effect.void;
  };

  const writeContent = (
    operation: string,
    path: string,
    content: string,
  ): Effect.Effect<void, FileSystemError> =>
    queryVoid({
      operation,
      method: "put",
      type: "content",
      body: { path, content } satisfies WriteRequest,
    });

  const writeFileString = ((path: string, data: string, writeOptions) =>
    guardWrite("writeFileString", writeOptions).pipe(
      Effect.flatMap(() => writeContent("writeFileString", path, data)),
    )) satisfies FileSystem["writeFileString"];

  // The mount stores text, so bytes are written only when they decode as UTF-8;
  // a binary write belongs on the upload mount rather than here.
  const writeFile = ((path: string, data: Uint8Array, writeOptions) =>
    guardWrite("writeFile", writeOptions).pipe(
      Effect.flatMap(() =>
        Effect.try({
          try: () => new TextDecoder("utf-8", { fatal: true }).decode(data),
          catch: (cause) =>
            PlatformError.badArgument({
              module: "FileSystem",
              method: "writeFile",
              description: `content is not UTF-8: ${path}`,
              cause,
            }),
        }),
      ),
      Effect.flatMap((content) => writeContent("writeFile", path, content)),
    )) satisfies FileSystem["writeFile"];

  // The mount stores text only, so bytes go through the upload mount, which
  // stages them until `commit` moves them onto the path. The sink hands its
  // input to the upload as a stream instead of buffering it, so memory stays
  // bounded by the tus chunk size; the length is deferred because a sink does
  // not know how many bytes it will receive, and a failed `commit` leaves the
  // staged upload for a later one.
  const sink = ((path: string, sinkOptions) =>
    Sink.fromTransform<Uint8Array, void, FileSystemError, never>((upstream) =>
      Effect.gen(function* () {
        yield* guardWrite("sink", sinkOptions);
        yield* guardPaths("sink", { path });

        // A leading zero-length chunk keeps the source from ending before it
        // holds any bytes: tus cannot slice such a stream, so an empty sink
        // would fail instead of creating an empty upload.
        const bytes = Stream.fromPull(Effect.succeed(upstream)).pipe(
          Stream.prepend([new Uint8Array(0)]),
        );

        const upload = yield* tus
          .uploadResult({
            source: Stream.toReadableStream(bytes),
            chunkSize: sinkChunkSize,
            uploadLengthDeferred: true,
          })
          .pipe(Effect.mapError(toPlatformUploadError("sink", path)));

        yield* queryVoid({
          operation: "sink",
          method: "post",
          type: "commit",
          body: { upload: uploadId(upload.url), path } satisfies CommitRequest,
        });

        return [undefined];
      }),
    )) satisfies FileSystem["sink"];

  const fails = (method: string): Effect.Effect<never, FileSystemError> =>
    Effect.fail(unsupported(method));

  return FileSystem.of({
    access,
    chmod,
    copy,
    copyFile,
    glob,
    exists,
    symlink,
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
    realPath,
    remove,
    rename,
    sink,
    stat,
    stream,
    truncate,
    utimes,
    watch,
    writeFile,
    writeFileString,
  });
});

/** The file system service in direct mode, where paths are absolute. */
export const layer = (config: { readonly baseUrl?: string | URL | undefined } = {}) =>
  Layer.effect(FileSystem, make()).pipe(
    Layer.provide(Layer.mergeAll(clientLayer(config), tusLayer(config))),
  );
