import { Context, Data, Effect, Layer, Option, Schema, Stream } from "effect";
import {
  FetchHttpClient,
  HttpClient,
  HttpClientError,
  HttpClientRequest,
  HttpClientResponse,
} from "effect/unstable/http";

/**
 * Failure before the server produced a response: the connection could not be
 * opened, the request could not be encoded, or the response body could not be
 * read.
 */
export class TransportError extends Data.TaggedError("TransportError")<{
  readonly message: string;
  readonly cause: unknown;
}> {}

/**
 * Failure reported by the server through the shared JSON error envelope, so the
 * stable `code` survives regardless of the transport that carried it.
 */
export class ApiError extends Data.TaggedError("ApiError")<{
  readonly status: number;
  readonly code: string;
  readonly message: string;
  readonly requestId: string | undefined;
}> {}

export type ClientError = TransportError | ApiError;

/** Request body of the shared error envelope, see the FileSystem error protocol. */
const errorResponseSchema = Schema.Struct({
  error: Schema.Struct({
    code: Schema.String,
    message: Schema.String,
    request_id: Schema.String,
  }),
});

export interface ClientConfig {
  /** Base URL prepended to every request URL. */
  readonly baseUrl?: string | URL | undefined;
}

export interface Client {
  /**
   * Sends a request and returns the `2xx` response, exposing status, `ETag`,
   * and `Last-Modified` to the callers that need them.
   */
  readonly execute: (
    request: HttpClientRequest.HttpClientRequest,
  ) => Effect.Effect<HttpClientResponse.HttpClientResponse, ClientError>;

  /** Sends a request and decodes the response body as JSON. */
  readonly json: <A = Schema.Json>(
    request: HttpClientRequest.HttpClientRequest,
  ) => Effect.Effect<A, ClientError>;

  /** Sends a request and decodes the response body as text. */
  readonly text: (
    request: HttpClientRequest.HttpClientRequest,
  ) => Effect.Effect<string, ClientError>;

  /** Sends a request and decodes the response body as bytes. */
  readonly bytes: (
    request: HttpClientRequest.HttpClientRequest,
  ) => Effect.Effect<Uint8Array, ClientError>;

  /** Sends a request and discards the response body. */
  readonly void: (request: HttpClientRequest.HttpClientRequest) => Effect.Effect<void, ClientError>;

  /** Sends a request and streams the response body. */
  readonly stream: (
    request: HttpClientRequest.HttpClientRequest,
  ) => Stream.Stream<Uint8Array, ClientError>;
}

export const Client: Context.Service<Client, Client> = Context.Service("sandbox-toolkit/Client");

/** Wraps a failure to read a response body, for callers decoding it themselves. */
export const transportError = (error: HttpClientError.HttpClientError): TransportError =>
  new TransportError({ message: error.message, cause: error });

const mapBodyError = <A, R>(
  effect: Effect.Effect<A, HttpClientError.HttpClientError, R>,
): Effect.Effect<A, TransportError, R> =>
  Effect.catchTag(effect, "HttpClientError", (error) => Effect.fail(transportError(error)));

const decodeError = (
  response: HttpClientResponse.HttpClientResponse,
): Effect.Effect<ApiError, never> =>
  Effect.gen(function* () {
    const decoded = yield* Effect.option(
      HttpClientResponse.schemaBodyJson(errorResponseSchema)(response),
    );
    const details = Option.getOrUndefined(decoded)?.error;

    return new ApiError({
      status: response.status,
      code: details?.code ?? `http_${response.status}`,
      message: details?.message ?? `request failed with status ${response.status}`,
      requestId: details?.request_id,
    });
  });

const toClientError = (
  error: HttpClientError.HttpClientError,
): Effect.Effect<never, ClientError> =>
  error.reason._tag === "StatusCodeError"
    ? Effect.flatMap(decodeError(error.reason.response), Effect.fail)
    : Effect.fail(transportError(error));

export const make = (
  config: ClientConfig = {},
): Effect.Effect<Client, never, HttpClient.HttpClient> =>
  Effect.gen(function* () {
    const httpClient = yield* HttpClient.HttpClient;
    const client = httpClient.pipe(
      HttpClient.mapRequest(HttpClientRequest.prependUrl(config.baseUrl?.toString() ?? "")),
      HttpClient.filterStatusOk,
    );

    const execute = (
      request: HttpClientRequest.HttpClientRequest,
    ): Effect.Effect<HttpClientResponse.HttpClientResponse, ClientError> =>
      Effect.catchTag(client.execute(request), "HttpClientError", toClientError);

    return Client.of({
      execute,

      json: <A = Schema.Json>(request: HttpClientRequest.HttpClientRequest) =>
        Effect.gen(function* () {
          const response = yield* execute(request);

          return (yield* mapBodyError(response.json)) as A;
        }),

      text: (request) =>
        Effect.gen(function* () {
          const response = yield* execute(request);

          return yield* mapBodyError(response.text);
        }),

      bytes: (request) =>
        Effect.gen(function* () {
          const response = yield* execute(request);

          return new Uint8Array(yield* mapBodyError(response.arrayBuffer));
        }),

      void: (request) => Effect.asVoid(execute(request)),

      stream: (request) =>
        Stream.unwrap(
          Effect.map(execute(request), (response) =>
            Stream.mapError(response.stream, transportError),
          ),
        ),
    });
  });

export const layer = (
  config: ClientConfig = {},
): Layer.Layer<Client, never, HttpClient.HttpClient> => Layer.effect(Client, make(config));

/** Convenience layer for the callers that do not provide an `HttpClient` themselves. */
export const layerFetch = (config: ClientConfig = {}): Layer.Layer<Client> =>
  Layer.provide(layer(config), FetchHttpClient.layer);
