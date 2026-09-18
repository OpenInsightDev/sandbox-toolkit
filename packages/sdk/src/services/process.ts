import { Context, Effect, Layer } from "effect";
import {
  FetchHttpClient,
  HttpClient,
  HttpClientRequest,
  HttpClientResponse,
} from "effect/unstable/http";

import type { SandboxToolkitError } from "../errors.ts";
import {
  ExecResultSchema,
  type ExecParams,
  type ExecResult,
  type ShellParams,
} from "../schemas.ts";
import { ApiClient, type Transport } from "./api-client.ts";

export interface ProcessService {
  readonly exec: (
    command: string,
    options?: Omit<ExecParams, "command">,
  ) => Effect.Effect<ExecResult, SandboxToolkitError>;
  readonly shell: (
    script: string,
    options?: Omit<ShellParams, "script">,
  ) => Effect.Effect<ExecResult, SandboxToolkitError>;
}

export const makeProcess = (transport: Transport): ProcessService => ({
  exec: (command, options) =>
    transport
      .execute(
        HttpClientRequest.post("/process/exec").pipe(
          HttpClientRequest.bodyJsonUnsafe({ command, ...options }),
        ),
        HttpClientResponse.schemaBodyJson(ExecResultSchema),
      )
      .pipe(Effect.withSpan("SandboxToolkit.exec")),
  shell: (script, options) =>
    transport
      .execute(
        HttpClientRequest.post("/process/shell").pipe(
          HttpClientRequest.bodyJsonUnsafe({ script, ...options }),
        ),
        HttpClientResponse.schemaBodyJson(ExecResultSchema),
      )
      .pipe(Effect.withSpan("SandboxToolkit.shell")),
});

export class Process extends Context.Service<Process, ProcessService>()(
  "@sandbox-toolkit/sdk/services/Process",
) {
  static readonly layerNoDeps = (options: {
    readonly baseUrl: string;
  }): Layer.Layer<Process, never, HttpClient.HttpClient> =>
    Layer.effect(
      Process,
      Effect.gen(function* () {
        const transport = yield* ApiClient;
        return makeProcess(transport);
      }),
    ).pipe(Layer.provide(ApiClient.layerNoDeps(options)));

  static readonly layer = (options: { readonly baseUrl: string }): Layer.Layer<Process> =>
    Process.layerNoDeps(options).pipe(Layer.provide(FetchHttpClient.layer));
}
