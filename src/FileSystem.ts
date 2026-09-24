import {
  Context,
  Effect,
  PlatformError,
  type ByteSize,
  type Scope,
  type Sink,
  type Stream,
} from "effect";
import type { OpenFlag, File } from "effect/FileSystem";

export type FileSystemError = PlatformError.PlatformError;

export interface FileSystem {
  readonly access: (path: string) => Effect.Effect<void, FileSystemError>;

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
