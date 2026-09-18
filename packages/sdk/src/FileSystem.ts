import { Context, Effect, Layer } from "effect";
import {
  FetchHttpClient,
  HttpClient,
  HttpClientRequest,
  HttpClientResponse,
} from "effect/unstable/http";

import type { SandboxToolkitError } from "./SandboxToolkitError.ts";
import {
  CopyResultSchema,
  ListResultSchema,
  MkdirResultSchema,
  MoveResultSchema,
  ReadFileResultSchema,
  RemoveResultSchema,
  StatResultSchema,
  WriteFileResultSchema,
  type CopyParams,
  type CopyResult,
  type ListResult,
  type MkdirParams,
  type MkdirResult,
  type MoveParams,
  type MoveResult,
  type ReadFileParams,
  type ReadFileResult,
  type RemoveParams,
  type RemoveResult,
  type StatResult,
  type WriteFileParams,
  type WriteFileResult,
} from "./Schemas.ts";
import { ApiClient, type Transport } from "./internal/apiClient.ts";

export interface FileSystemService {
  readonly readFile: (
    path: string,
    options?: Omit<ReadFileParams, "path">,
  ) => Effect.Effect<ReadFileResult, SandboxToolkitError>;
  readonly stat: (path: string) => Effect.Effect<StatResult, SandboxToolkitError>;
  readonly list: (path: string) => Effect.Effect<ListResult, SandboxToolkitError>;
  readonly mkdir: (
    path: string,
    options?: Omit<MkdirParams, "path">,
  ) => Effect.Effect<MkdirResult, SandboxToolkitError>;
  readonly writeFile: (
    path: string,
    contents: string,
    options?: Omit<WriteFileParams, "path" | "contents">,
  ) => Effect.Effect<WriteFileResult, SandboxToolkitError>;
  readonly remove: (
    path: string,
    options?: Omit<RemoveParams, "path">,
  ) => Effect.Effect<RemoveResult, SandboxToolkitError>;
  readonly copy: (
    source: string,
    destination: string,
    options?: Omit<CopyParams, "source" | "destination">,
  ) => Effect.Effect<CopyResult, SandboxToolkitError>;
  readonly move: (
    source: string,
    destination: string,
    options?: Omit<MoveParams, "source" | "destination">,
  ) => Effect.Effect<MoveResult, SandboxToolkitError>;
}

export const makeFileSystem = (transport: Transport): FileSystemService => ({
  readFile: (path, options) =>
    transport
      .execute(
        HttpClientRequest.post("/fs/readFile").pipe(
          HttpClientRequest.bodyJsonUnsafe({ path, ...options }),
        ),
        HttpClientResponse.schemaBodyJson(ReadFileResultSchema),
      )
      .pipe(Effect.withSpan("SandboxToolkit.readFile")),
  stat: (path) =>
    transport
      .execute(
        HttpClientRequest.post("/fs/stat").pipe(HttpClientRequest.bodyJsonUnsafe({ path })),
        HttpClientResponse.schemaBodyJson(StatResultSchema),
      )
      .pipe(Effect.withSpan("SandboxToolkit.stat")),
  list: (path) =>
    transport
      .execute(
        HttpClientRequest.post("/fs/list").pipe(HttpClientRequest.bodyJsonUnsafe({ path })),
        HttpClientResponse.schemaBodyJson(ListResultSchema),
      )
      .pipe(Effect.withSpan("SandboxToolkit.list")),
  mkdir: (path, options) =>
    transport
      .execute(
        HttpClientRequest.post("/fs/mkdir").pipe(
          HttpClientRequest.bodyJsonUnsafe({ path, ...options }),
        ),
        HttpClientResponse.schemaBodyJson(MkdirResultSchema),
      )
      .pipe(Effect.withSpan("SandboxToolkit.mkdir")),
  writeFile: (path, contents, options) =>
    transport
      .execute(
        HttpClientRequest.post("/fs/writeFile").pipe(
          HttpClientRequest.bodyJsonUnsafe({ path, contents, ...options }),
        ),
        HttpClientResponse.schemaBodyJson(WriteFileResultSchema),
      )
      .pipe(Effect.withSpan("SandboxToolkit.writeFile")),
  remove: (path, options) =>
    transport
      .execute(
        HttpClientRequest.post("/fs/remove").pipe(
          HttpClientRequest.bodyJsonUnsafe({ path, ...options }),
        ),
        HttpClientResponse.schemaBodyJson(RemoveResultSchema),
      )
      .pipe(Effect.withSpan("SandboxToolkit.remove")),
  copy: (source, destination, options) =>
    transport
      .execute(
        HttpClientRequest.post("/fs/copy").pipe(
          HttpClientRequest.bodyJsonUnsafe({ source, destination, ...options }),
        ),
        HttpClientResponse.schemaBodyJson(CopyResultSchema),
      )
      .pipe(Effect.withSpan("SandboxToolkit.copy")),
  move: (source, destination, options) =>
    transport
      .execute(
        HttpClientRequest.post("/fs/move").pipe(
          HttpClientRequest.bodyJsonUnsafe({ source, destination, ...options }),
        ),
        HttpClientResponse.schemaBodyJson(MoveResultSchema),
      )
      .pipe(Effect.withSpan("SandboxToolkit.move")),
});

export class FileSystem extends Context.Service<FileSystem, FileSystemService>()(
  "@sandbox-toolkit/sdk/FileSystem",
) {
  static readonly layerNoDeps = (options: {
    readonly baseUrl: string;
  }): Layer.Layer<FileSystem, never, HttpClient.HttpClient> =>
    Layer.effect(
      FileSystem,
      Effect.gen(function* () {
        const transport = yield* ApiClient;
        return makeFileSystem(transport);
      }),
    ).pipe(Layer.provide(ApiClient.layerNoDeps(options)));

  static readonly layer = (options: { readonly baseUrl: string }): Layer.Layer<FileSystem> =>
    FileSystem.layerNoDeps(options).pipe(Layer.provide(FetchHttpClient.layer));
}
