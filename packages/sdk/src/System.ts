import { Context, Effect, Layer } from "effect";
import {
  FetchHttpClient,
  HttpClient,
  HttpClientRequest,
  HttpClientResponse,
} from "effect/unstable/http";

import type { SandboxToolkitError } from "./SandboxToolkitError.ts";
import { HealthSchema, type HealthResult } from "./Schemas.ts";
import { ApiClient, type Transport } from "./internal/apiClient.ts";

export interface SystemService {
  readonly health: Effect.Effect<HealthResult, SandboxToolkitError>;
}

export const makeSystem = (transport: Transport): SystemService => ({
  health: transport
    .execute(HttpClientRequest.get("/health"), HttpClientResponse.schemaBodyJson(HealthSchema))
    .pipe(Effect.withSpan("SandboxToolkit.health")),
});

export class System extends Context.Service<System, SystemService>()(
  "@sandbox-toolkit/sdk/System",
) {
  static readonly layerNoDeps = (options: {
    readonly baseUrl: string;
  }): Layer.Layer<System, never, HttpClient.HttpClient> =>
    Layer.effect(
      System,
      Effect.gen(function* () {
        const transport = yield* ApiClient;
        return makeSystem(transport);
      }),
    ).pipe(Layer.provide(ApiClient.layerNoDeps(options)));

  static readonly layer = (options: { readonly baseUrl: string }): Layer.Layer<System> =>
    System.layerNoDeps(options).pipe(Layer.provide(FetchHttpClient.layer));
}
