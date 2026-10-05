import { PluginNotFound, type ListPluginsOptions, type PluginError } from "../Plugin.ts";
import { ApiError, type ClientError } from "./client.ts";
import { route } from "./prelude.ts";

/** The collection URL, with only the paging options that were given. */
export const listUrl = (workspace: string | undefined, options?: ListPluginsOptions): string => {
  const base = route(workspace, "/plugins");
  const params = new URLSearchParams();

  if (options?.offset !== undefined) params.set("offset", `${options.offset}`);

  if (options?.limit !== undefined) params.set("limit", `${options.limit}`);

  const query = params.toString();

  return query === "" ? base : `${base}?${query}`;
};

export const itemUrl = (workspace: string | undefined, pluginId: string): string =>
  `${route(workspace, "/plugins")}/${pluginId}`;

/** A missing plugin is a domain error; every other transport failure survives. */
export const toLookupError = (pluginId: string, error: ClientError): PluginError =>
  error instanceof ApiError && error.code === "not_found"
    ? new PluginNotFound({ pluginId })
    : error;
