import { Context, Effect, Layer } from "effect";
import {
  FetchHttpClient,
  HttpClient,
  HttpClientRequest,
  HttpClientResponse,
} from "effect/unstable/http";

import type { SandboxToolkitError } from "./SandboxToolkitError.ts";
import {
  GetSkillResultSchema,
  ListSkillsResultSchema,
  type GetSkillResult,
  type Skill,
} from "./Schemas.ts";
import { ApiClient, type Transport } from "./internal/apiClient.ts";

export interface SkillsService {
  readonly listSkills: Effect.Effect<ReadonlyArray<Skill>, SandboxToolkitError>;
  readonly getSkill: (name: string) => Effect.Effect<GetSkillResult, SandboxToolkitError>;
}

export const makeSkills = (transport: Transport): SkillsService => ({
  listSkills: transport
    .execute(
      HttpClientRequest.get("/skills"),
      HttpClientResponse.schemaBodyJson(ListSkillsResultSchema),
    )
    .pipe(
      Effect.map((result) => result.skills),
      Effect.withSpan("SandboxToolkit.listSkills"),
    ),
  getSkill: (name) =>
    transport
      .execute(
        HttpClientRequest.post("/skills/get").pipe(HttpClientRequest.bodyJsonUnsafe({ name })),
        HttpClientResponse.schemaBodyJson(GetSkillResultSchema),
      )
      .pipe(Effect.withSpan("SandboxToolkit.getSkill")),
});

export class Skills extends Context.Service<Skills, SkillsService>()(
  "@sandbox-toolkit/sdk/Skills",
) {
  static readonly layerNoDeps = (options: {
    readonly baseUrl: string;
  }): Layer.Layer<Skills, never, HttpClient.HttpClient> =>
    Layer.effect(
      Skills,
      Effect.gen(function* () {
        const transport = yield* ApiClient;
        return makeSkills(transport);
      }),
    ).pipe(Layer.provide(ApiClient.layerNoDeps(options)));

  static readonly layer = (options: { readonly baseUrl: string }): Layer.Layer<Skills> =>
    Skills.layerNoDeps(options).pipe(Layer.provide(FetchHttpClient.layer));
}
