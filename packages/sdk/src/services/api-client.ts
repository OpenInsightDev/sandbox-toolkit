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
} from "../errors.ts";

export interface Transport {
  readonly execute: <A>(
    request: HttpClientRequest.HttpClientRequest,
    decodeBody: (response: HttpClientResponse.HttpClientResponse) => Effect.Effect<A, unknown>,
  ) => Effect.Effect<A, SandboxToolkitError>;
}

const transportError = (cause: unknown): SandboxToolkitError =>
  new SandboxToolkitError({
    reason: new TransportError({
      message: cause instanceof Error ? cause.message : String(cause),
      cause,
    }),
  });

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

const makeTransport = (base: HttpClient.HttpClient, baseUrl: string): Transport => {
  const client = base.pipe(
    HttpClient.mapRequest(
      flow(HttpClientRequest.prependUrl(baseUrl), HttpClientRequest.acceptJson),
    ),
  );

  const execute = Effect.fnUntraced(function* <A>(
    request: HttpClientRequest.HttpClientRequest,
    decodeBody: (response: HttpClientResponse.HttpClientResponse) => Effect.Effect<A, unknown>,
  ): Effect.fn.Return<A, SandboxToolkitError> {
    const response = yield* client.execute(request).pipe(Effect.mapError(transportError));

    if (response.status < 200 || response.status >= 300) {
      const body = yield* response.text.pipe(
        Effect.catch((error) =>
          Effect.succeed(`HTTP ${response.status} (unreadable body: ${error.message})`),
        ),
      );
      return yield* Effect.fail(fromStatus(response.status, body));
    }

    return yield* decodeBody(response).pipe(Effect.mapError(transportError));
  });

  return { execute };
};

export class ApiClient extends Context.Service<ApiClient, Transport>()(
  "@sandbox-toolkit/sdk/services/ApiClient",
) {
  static readonly layerNoDeps = (options: {
    readonly baseUrl: string;
  }): Layer.Layer<ApiClient, never, HttpClient.HttpClient> =>
    Layer.effect(
      ApiClient,
      Effect.gen(function* () {
        const base = yield* HttpClient.HttpClient;
        return makeTransport(base, options.baseUrl);
      }),
    );

  static readonly layer = (options: { readonly baseUrl: string }): Layer.Layer<ApiClient> =>
    ApiClient.layerNoDeps(options).pipe(Layer.provide(FetchHttpClient.layer));
}
