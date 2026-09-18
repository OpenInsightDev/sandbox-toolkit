import { Context, Effect, Layer } from "effect";
import {
  FetchHttpClient,
  HttpClient,
  HttpClientRequest,
  HttpClientResponse,
} from "effect/unstable/http";

import type { SandboxToolkitError } from "../errors.ts";
import {
  DescribeToolResultSchema,
  ListToolsResultSchema,
  type DescribeToolResult,
} from "../schemas.ts";
import { ApiClient, type Transport } from "./api-client.ts";

export interface ToolsService {
  readonly listTools: Effect.Effect<ReadonlyArray<DescribeToolResult>, SandboxToolkitError>;
  readonly describeTool: (name: string) => Effect.Effect<DescribeToolResult, SandboxToolkitError>;
}

export const makeTools = (transport: Transport): ToolsService => ({
  listTools: transport
    .execute(
      HttpClientRequest.get("/tools"),
      HttpClientResponse.schemaBodyJson(ListToolsResultSchema),
    )
    .pipe(
      Effect.map((result) => result.tools),
      Effect.withSpan("SandboxToolkit.listTools"),
    ),
  describeTool: (name) =>
    transport
      .execute(
        HttpClientRequest.post("/tools/describe").pipe(HttpClientRequest.bodyJsonUnsafe({ name })),
        HttpClientResponse.schemaBodyJson(DescribeToolResultSchema),
      )
      .pipe(Effect.withSpan("SandboxToolkit.describeTool")),
});

export class Tools extends Context.Service<Tools, ToolsService>()(
  "@sandbox-toolkit/sdk/services/Tools",
) {
  static readonly layerNoDeps = (options: {
    readonly baseUrl: string;
  }): Layer.Layer<Tools, never, HttpClient.HttpClient> =>
    Layer.effect(
      Tools,
      Effect.gen(function* () {
        const transport = yield* ApiClient;
        return makeTools(transport);
      }),
    ).pipe(Layer.provide(ApiClient.layerNoDeps(options)));

  static readonly layer = (options: { readonly baseUrl: string }): Layer.Layer<Tools> =>
    Tools.layerNoDeps(options).pipe(Layer.provide(FetchHttpClient.layer));
}
