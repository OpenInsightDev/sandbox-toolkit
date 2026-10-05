import { Context, Data, Effect, Layer } from "effect";
import { HttpClientRequest } from "effect/unstable/http";

import type { PluginMetadata } from "./generated/PluginMetadata.ts";
import { Client, layer as clientLayer, type ClientError } from "./internal/client.ts";
import { itemUrl, listUrl, toLookupError } from "./internal/plugin.ts";

export type { PluginMetadata } from "./generated/PluginMetadata.ts";

/** No plugin is discovered under the id at the mount point. */
export class PluginNotFound extends Data.TaggedError("PluginNotFound")<{
  readonly pluginId: string;
}> {}

export type PluginError = ClientError | PluginNotFound;

export interface ListPluginsOptions {
  /** Zero-based index of the first plugin to return, in id order. */
  readonly offset?: number;
  /** Maximum number of plugins to return. */
  readonly limit?: number;
}

export interface Plugin {
  /**
   * The plugins discovered at the mount point, in id order. The mount point is
   * rescanned per call, so directory changes are visible immediately.
   */
  readonly list: (
    options?: ListPluginsOptions,
  ) => Effect.Effect<ReadonlyArray<PluginMetadata>, PluginError>;

  /** The plugin registered under the id at the mount point. */
  readonly get: (pluginId: string) => Effect.Effect<PluginMetadata, PluginError>;
}

export const Plugin: Context.Service<Plugin, Plugin> = Context.Service("plugin");

export const make = Effect.fn("Plugin.make")(function* (
  mount: { workspace?: string | undefined } = {},
) {
  const client = yield* Client;

  const list = ((options?: ListPluginsOptions) =>
    client.json<ReadonlyArray<PluginMetadata>>(
      HttpClientRequest.get(listUrl(mount.workspace, options)),
    )) satisfies Plugin["list"];

  const get = Effect.fn("Plugin.get")(function* (pluginId: string) {
    return yield* client
      .json<PluginMetadata>(HttpClientRequest.get(itemUrl(mount.workspace, pluginId)))
      .pipe(Effect.mapError((error) => toLookupError(pluginId, error)));
  }) satisfies Plugin["get"];

  return Plugin.of({ list, get });
});

/** The plugin service over the global mount point. */
export const layer = (config: { readonly baseUrl?: string | URL | undefined } = {}) =>
  Layer.effect(Plugin, make()).pipe(Layer.provide(clientLayer(config)));
