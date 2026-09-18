import { Context, Effect, Layer } from "effect";
import { FetchHttpClient, HttpClient } from "effect/unstable/http";

import { ApiClient } from "./services/api-client.ts";
import { makeFileSystem, type FileSystemService } from "./services/filesystem.ts";
import { makeProcess, type ProcessService } from "./services/process.ts";
import { makeSystem, type SystemService } from "./services/system.ts";
import { makeTools, type ToolsService } from "./services/tools.ts";

export interface Service extends SystemService, ToolsService, FileSystemService, ProcessService {}

export class SandboxToolkit extends Context.Service<SandboxToolkit, Service>()(
  "@sandbox-toolkit/sdk/client/SandboxToolkit",
) {
  static readonly layerNoDeps = (options: {
    readonly baseUrl: string;
  }): Layer.Layer<SandboxToolkit, never, HttpClient.HttpClient> =>
    Layer.effect(
      SandboxToolkit,
      Effect.gen(function* () {
        const transport = yield* ApiClient;
        return SandboxToolkit.of({
          ...makeSystem(transport),
          ...makeTools(transport),
          ...makeFileSystem(transport),
          ...makeProcess(transport),
        });
      }),
    ).pipe(Layer.provide(ApiClient.layerNoDeps(options)));

  static readonly layer = (options: { readonly baseUrl: string }): Layer.Layer<SandboxToolkit> =>
    SandboxToolkit.layerNoDeps(options).pipe(Layer.provide(FetchHttpClient.layer));
}
