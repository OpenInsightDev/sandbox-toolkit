import { Schema } from "effect";

/**
 * Agent Plugins `mcp.json` 1.0.0, following
 * https://agent-plugins.org/schemas/1.0.0/mcp.schema.json.
 */

/** `$schema` identifier fixed by Agent Plugins 1.0.0 for `mcp.json`. */
export const MCP_SCHEMA_1_0_0 = "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json";

/** HTTP headers sent to a remote MCP endpoint; literal, non-sensitive data. */
export const Headers = Schema.Record(Schema.String, Schema.String);

/**
 * Variables the client injects into a stdio child; an `env` entry redeclaring one
 * would shadow the value the server controls, so its name is rejected.
 */
const reservedPluginVariables: ReadonlySet<string> = new Set(["PLUGIN_ROOT", "PLUGIN_DATA"]);

/** Extra environment of a stdio child, keyed by variable name. */
export const StdioEnv = Schema.Record(
  Schema.String.check(
    Schema.makeFilter((name) => !reservedPluginVariables.has(name), {
      message: "must not redeclare a reserved plugin variable",
    }),
  ),
  Schema.String,
);

/**
 * A plugin-relative `cwd`: `./`, `${PLUGIN_ROOT}`, or `${PLUGIN_DATA}` rooted.
 * Filesystem containment is validated separately by the server.
 */
export const PluginRelativePath = Schema.String.check(
  Schema.isPattern(/^(?:\.\/|\$\{PLUGIN_ROOT\}(?:\/|$)|\$\{PLUGIN_DATA\}(?:\/|$))/),
);

/** stdio MCP server: a local executable the server launches per session. */
export const StdioServer = Schema.Struct({
  type: Schema.Literal("stdio"),
  command: Schema.NonEmptyString,
  args: Schema.optionalKey(Schema.Array(Schema.String)),
  env: Schema.optionalKey(StdioEnv),
  cwd: Schema.optionalKey(PluginRelativePath),
});

/** Streamable HTTP MCP server: a remote endpoint the server proxies. */
export const StreamableHttpServer = Schema.Struct({
  type: Schema.Literal("streamable-http"),
  url: Schema.NonEmptyString,
  headers: Schema.optionalKey(Headers),
});

/** Legacy HTTP+SSE MCP server, kept for compatibility. */
export const SseServer = Schema.Struct({
  type: Schema.Literal("sse"),
  url: Schema.NonEmptyString,
  headers: Schema.optionalKey(Headers),
});

/** One `mcpServers` entry, discriminated by its `type` tag. */
export const Server = Schema.Union([StdioServer, StreamableHttpServer, SseServer]);

/**
 * An Agent Plugins 1.0.0 `mcp.json` document. The schema is closed: decode with
 * `onExcessProperty: "error"`, since Effect otherwise strips unmodeled keys.
 */
export const McpConfig = Schema.Struct({
  $schema: Schema.Literal(MCP_SCHEMA_1_0_0),
  mcpServers: Schema.Record(Schema.String, Server),
});
