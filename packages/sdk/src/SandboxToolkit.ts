import { Context, Effect, Layer } from "effect";
import { FetchHttpClient, HttpClient } from "effect/unstable/http";

import { ApiClient } from "./internal/apiClient.ts";
import { makeFileSystem, type FileSystemService } from "./FileSystem.ts";
import { makeProcess, type ProcessService } from "./Process.ts";
import { makeSystem, type SystemService } from "./System.ts";
import { makeTools, type ToolsService } from "./Tools.ts";

export interface Service extends SystemService, ToolsService, FileSystemService, ProcessService {}

export class SandboxToolkit extends Context.Service<SandboxToolkit, Service>()(
  "@sandbox-toolkit/sdk/SandboxToolkit",
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
