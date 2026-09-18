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
  DescribeToolResultSchema,
  HealthSchema,
  ListToolsResultSchema,
  ReadFileResultSchema,
  type DescribeToolParams,
  type DescribeToolResult,
  type HealthResult,
  type ReadFileParams,
  type ReadFileResult,
} from "./schemas.ts";

/** The operations exposed by the server's HTTP surface. */
export interface Service {
  readonly health: Effect.Effect<HealthResult, SandboxToolkitError>;
  readonly listTools: Effect.Effect<ReadonlyArray<DescribeToolResult>, SandboxToolkitError>;
  readonly describeTool: (
    params: DescribeToolParams,
  ) => Effect.Effect<DescribeToolResult, SandboxToolkitError>;
  readonly readFile: (params: ReadFileParams) => Effect.Effect<ReadFileResult, SandboxToolkitError>;
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

        return SandboxToolkit.of({
          health,
          listTools,
          describeTool,
          readFile,
        });
      }),
    );

  /** A fully-wired layer backed by the platform `fetch` implementation. */
  static readonly layer = (options: { readonly baseUrl: string }): Layer.Layer<SandboxToolkit> =>
    SandboxToolkit.layerNoDeps(options).pipe(Layer.provide(FetchHttpClient.layer));
}
