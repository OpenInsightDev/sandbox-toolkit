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

import type { DirectoryResponse } from "./generated/DirectoryResponse.ts";
import type { ResourceMetadata } from "./generated/ResourceMetadata.ts";
import { Client } from "./internal/client.ts";
import {
  isNotFound,
  joinPath,
  metadataInfo,
  resourceUrl,
  toPlatformError,
  unsupported,
} from "./internal/filesystem.ts";
import { endLines, takeLines } from "./internal/process.ts";

export type FileSystemError = PlatformError.PlatformError;

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

  const url = (method: string, path: string) => resourceUrl(options.workspace, method, path);

  const queryJson = <A>(
    method: string,
    path: string,
    params: URLSearchParams,
  ): Effect.Effect<A, FileSystemError> =>
    Effect.flatMap(url(method, path), (target) =>
      Effect.mapError(
        client.json<A>(HttpClientRequest.query(`${target}?${params}`)),
        toPlatformError(method, path),
      ),
    );

  const readFile = ((path: string) =>
    Effect.flatMap(url("readFile", path), (target) =>
      Effect.mapError(
        client.bytes(HttpClientRequest.get(target)),
        toPlatformError("readFile", path),
      ),
    )) satisfies FileSystem["readFile"];

  const stream = ((path: string, streamOptions) => {
    if (streamOptions?.offset !== undefined || streamOptions?.bytesToRead !== undefined) {
      return Stream.fail(unsupported("stream(offset/bytesToRead)"));
    }

    return Stream.unwrap(
      Effect.map(url("stream", path), (target) =>
        Stream.mapError(
          client.stream(HttpClientRequest.get(`${target}?type=stream`)),
          toPlatformError("stream", path),
        ),
      ),
    );
  }) satisfies FileSystem["stream"];

  const stat = ((path: string) =>
    Effect.map(
      queryJson<ResourceMetadata>("stat", path, new URLSearchParams({ type: "metadata" })),
      metadataInfo,
    )) satisfies FileSystem["stat"];

  const exists = ((path: string) =>
    Effect.catchIf(Effect.as(stat(path), true), isNotFound, () =>
      Effect.succeed(false),
    )) satisfies FileSystem["exists"];

  const readDirectory = ((path: string, readOptions) =>
    Effect.map(
      queryJson<DirectoryResponse>(
        "readDirectory",
        path,
        readOptions?.recursive === true
          ? new URLSearchParams({ type: "list", depth: "infinity" })
          : new URLSearchParams({ type: "list" }),
      ),
      (directory) => directory.entries.map((entry) => joinPath(path, entry.name)),
    )) satisfies FileSystem["readDirectory"];

  const glob = ((pattern: string, globOptions) => {
    const root = globOptions?.root ?? "";
    const params = new URLSearchParams({ type: "glob", pattern });

    for (const exclude of globOptions?.exclude ?? []) {
      params.append("exclude", exclude);
    }

    // Matched entries carry the server's own paths, which only round-trip in
    // direct mode; the `name` component is reliable in both modes.
    return Effect.map(queryJson<DirectoryResponse>("glob", root, params), (directory) =>
      directory.entries.map((entry) => entry.path),
    );
  }) satisfies FileSystem["glob"];

  const makeDirectory = ((path: string, makeOptions) =>
    Effect.flatMap(url("makeDirectory", path), (target) => {
      const request = HttpClientRequest.put(`${target}?type=directory`).pipe(
        HttpClientRequest.setHeader("if-none-match", "*"),
        (self) =>
          makeOptions?.recursive === true
            ? HttpClientRequest.bodyJsonUnsafe(self, { recursive: true })
            : self,
      );

      return Effect.mapError(client.void(request), toPlatformError("makeDirectory", path));
    })) satisfies FileSystem["makeDirectory"];

  const readFileString = ((path: string, encoding?: string) =>
    Effect.map(readFile(path), (bytes) =>
      new TextDecoder(encoding ?? "utf-8").decode(bytes),
    )) satisfies FileSystem["readFileString"];

  const readLines = ((path: string) =>
    Stream.decodeText(stream(path, {})).pipe(
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
