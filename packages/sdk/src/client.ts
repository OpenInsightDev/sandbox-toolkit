import { Context, Effect, flow, Layer } from "effect";
import {
  FetchHttpClient,
  HttpClient,
  HttpClientRequest,
  HttpClientResponse,
} from "effect/unstable/http";

import {
  InvalidRequestError,
  NotFoundError,
  SandboxToolkitError,
  ServerError,
  TransportError,
} from "./errors.ts";
import {
  CopyResultSchema,
  DescribeToolResultSchema,
  ExecResultSchema,
  HealthSchema,
  ListResultSchema,
  ListToolsResultSchema,
  MkdirResultSchema,
  MoveResultSchema,
  ReadFileResultSchema,
  RemoveResultSchema,
  StatResultSchema,
  WriteFileResultSchema,
  type CopyParams,
  type CopyResult,
  type DescribeToolParams,
  type DescribeToolResult,
  type ExecParams,
  type ExecResult,
  type HealthResult,
  type ListParams,
  type ListResult,
  type MkdirParams,
  type MkdirResult,
  type MoveParams,
  type MoveResult,
  type ReadFileParams,
  type ReadFileResult,
  type RemoveParams,
  type RemoveResult,
  type StatParams,
  type StatResult,
  type WriteFileParams,
  type WriteFileResult,
} from "./schemas.ts";

/** The operations exposed by the server's HTTP surface. */
export interface Service {
  readonly health: Effect.Effect<HealthResult, SandboxToolkitError>;
  readonly listTools: Effect.Effect<ReadonlyArray<DescribeToolResult>, SandboxToolkitError>;
  readonly describeTool: (
    params: DescribeToolParams,
  ) => Effect.Effect<DescribeToolResult, SandboxToolkitError>;
  readonly readFile: (params: ReadFileParams) => Effect.Effect<ReadFileResult, SandboxToolkitError>;
  readonly stat: (params: StatParams) => Effect.Effect<StatResult, SandboxToolkitError>;
  readonly list: (params: ListParams) => Effect.Effect<ListResult, SandboxToolkitError>;
  readonly mkdir: (params: MkdirParams) => Effect.Effect<MkdirResult, SandboxToolkitError>;
  readonly writeFile: (
    params: WriteFileParams,
  ) => Effect.Effect<WriteFileResult, SandboxToolkitError>;
  readonly remove: (params: RemoveParams) => Effect.Effect<RemoveResult, SandboxToolkitError>;
  readonly copy: (params: CopyParams) => Effect.Effect<CopyResult, SandboxToolkitError>;
  readonly move: (params: MoveParams) => Effect.Effect<MoveResult, SandboxToolkitError>;
  readonly exec: (params: ExecParams) => Effect.Effect<ExecResult, SandboxToolkitError>;
}

const transportError = (cause: unknown): SandboxToolkitError =>
  new SandboxToolkitError({
    reason: new TransportError({
      message: cause instanceof Error ? cause.message : String(cause),
      cause,
    }),
  });

/** Mirror the server's status codes onto typed reasons. */
const fromStatus = (status: number, body: string): SandboxToolkitError => {
  if (status === 400) {
    return new SandboxToolkitError({
      reason: new InvalidRequestError({ message: body }),
    });
  }
  if (status === 404) {
    return new SandboxToolkitError({
      reason: new NotFoundError({ message: body }),
    });
  }
  return new SandboxToolkitError({
    reason: new ServerError({ status, message: body }),
  });
};

/**
 * An Effect client for the sandbox-toolkit HTTP server.
 *
 * Build one with `SandboxToolkit.layer({ baseUrl })` and provide it.
 */
export class SandboxToolkit extends Context.Service<SandboxToolkit, Service>()(
  "@sandbox-toolkit/sdk/client/SandboxToolkit",
) {
  /** A layer requiring an `HttpClient` (useful for tests and custom transports). */
  static readonly layerNoDeps = (options: {
    readonly baseUrl: string;
  }): Layer.Layer<SandboxToolkit, never, HttpClient.HttpClient> =>
    Layer.effect(
      SandboxToolkit,
      Effect.gen(function* () {
        const base = yield* HttpClient.HttpClient;
        const client = base.pipe(
          HttpClient.mapRequest(
            flow(HttpClientRequest.prependUrl(options.baseUrl), HttpClientRequest.acceptJson),
          ),
        );

        const execute = Effect.fnUntraced(function* <A>(
          request: HttpClientRequest.HttpClientRequest,
          decodeBody: (
            response: HttpClientResponse.HttpClientResponse,
          ) => Effect.Effect<A, unknown>,
        ): Effect.fn.Return<A, SandboxToolkitError> {
          const response = yield* client.execute(request).pipe(Effect.mapError(transportError));

          if (response.status < 200 || response.status >= 300) {
            // Keep the status when the plain-text error body cannot be read.
            const body = yield* response.text.pipe(
              Effect.catch((error) =>
                Effect.succeed(`HTTP ${response.status} (unreadable body: ${error.message})`),
              ),
            );
            return yield* Effect.fail(fromStatus(response.status, body));
          }

          return yield* decodeBody(response).pipe(Effect.mapError(transportError));
        });

        const health: Effect.Effect<HealthResult, SandboxToolkitError> = execute(
          HttpClientRequest.get("/health"),
          HttpClientResponse.schemaBodyJson(HealthSchema),
        ).pipe(Effect.withSpan("SandboxToolkit.health"));

        const listTools: Effect.Effect<
          ReadonlyArray<DescribeToolResult>,
          SandboxToolkitError
        > = execute(
          HttpClientRequest.get("/tools"),
          HttpClientResponse.schemaBodyJson(ListToolsResultSchema),
        ).pipe(
          Effect.map((result) => result.tools),
          Effect.withSpan("SandboxToolkit.listTools"),
        );

        const describeTool = (
          params: DescribeToolParams,
        ): Effect.Effect<DescribeToolResult, SandboxToolkitError> =>
          execute(
            HttpClientRequest.post("/tools/describe").pipe(
              HttpClientRequest.bodyJsonUnsafe(params),
            ),
            HttpClientResponse.schemaBodyJson(DescribeToolResultSchema),
          ).pipe(Effect.withSpan("SandboxToolkit.describeTool"));

        const readFile = (
          params: ReadFileParams,
        ): Effect.Effect<ReadFileResult, SandboxToolkitError> =>
          execute(
            HttpClientRequest.post("/fs/readFile").pipe(HttpClientRequest.bodyJsonUnsafe(params)),
            HttpClientResponse.schemaBodyJson(ReadFileResultSchema),
          ).pipe(Effect.withSpan("SandboxToolkit.readFile"));

        const stat = (params: StatParams): Effect.Effect<StatResult, SandboxToolkitError> =>
          execute(
            HttpClientRequest.post("/fs/stat").pipe(HttpClientRequest.bodyJsonUnsafe(params)),
            HttpClientResponse.schemaBodyJson(StatResultSchema),
          ).pipe(Effect.withSpan("SandboxToolkit.stat"));

        const list = (params: ListParams): Effect.Effect<ListResult, SandboxToolkitError> =>
          execute(
            HttpClientRequest.post("/fs/list").pipe(HttpClientRequest.bodyJsonUnsafe(params)),
            HttpClientResponse.schemaBodyJson(ListResultSchema),
          ).pipe(Effect.withSpan("SandboxToolkit.list"));

        const mkdir = (params: MkdirParams): Effect.Effect<MkdirResult, SandboxToolkitError> =>
          execute(
            HttpClientRequest.post("/fs/mkdir").pipe(HttpClientRequest.bodyJsonUnsafe(params)),
            HttpClientResponse.schemaBodyJson(MkdirResultSchema),
          ).pipe(Effect.withSpan("SandboxToolkit.mkdir"));

        const writeFile = (
          params: WriteFileParams,
        ): Effect.Effect<WriteFileResult, SandboxToolkitError> =>
          execute(
            HttpClientRequest.post("/fs/writeFile").pipe(HttpClientRequest.bodyJsonUnsafe(params)),
            HttpClientResponse.schemaBodyJson(WriteFileResultSchema),
          ).pipe(Effect.withSpan("SandboxToolkit.writeFile"));

        const remove = (params: RemoveParams): Effect.Effect<RemoveResult, SandboxToolkitError> =>
          execute(
            HttpClientRequest.post("/fs/remove").pipe(HttpClientRequest.bodyJsonUnsafe(params)),
            HttpClientResponse.schemaBodyJson(RemoveResultSchema),
          ).pipe(Effect.withSpan("SandboxToolkit.remove"));

        const copy = (params: CopyParams): Effect.Effect<CopyResult, SandboxToolkitError> =>
          execute(
            HttpClientRequest.post("/fs/copy").pipe(HttpClientRequest.bodyJsonUnsafe(params)),
            HttpClientResponse.schemaBodyJson(CopyResultSchema),
          ).pipe(Effect.withSpan("SandboxToolkit.copy"));

        const move = (params: MoveParams): Effect.Effect<MoveResult, SandboxToolkitError> =>
          execute(
            HttpClientRequest.post("/fs/move").pipe(HttpClientRequest.bodyJsonUnsafe(params)),
            HttpClientResponse.schemaBodyJson(MoveResultSchema),
          ).pipe(Effect.withSpan("SandboxToolkit.move"));

        const exec = (params: ExecParams): Effect.Effect<ExecResult, SandboxToolkitError> =>
          execute(
            HttpClientRequest.post("/process/exec").pipe(HttpClientRequest.bodyJsonUnsafe(params)),
            HttpClientResponse.schemaBodyJson(ExecResultSchema),
          ).pipe(Effect.withSpan("SandboxToolkit.exec"));

        return SandboxToolkit.of({
          health,
          listTools,
          describeTool,
          readFile,
          stat,
          list,
          mkdir,
          writeFile,
          remove,
          copy,
          move,
          exec,
        });
      }),
    );

  /** A fully-wired layer backed by the platform `fetch` implementation. */
  static readonly layer = (options: { readonly baseUrl: string }): Layer.Layer<SandboxToolkit> =>
    SandboxToolkit.layerNoDeps(options).pipe(Layer.provide(FetchHttpClient.layer));
}
