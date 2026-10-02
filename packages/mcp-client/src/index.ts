import {
  Client,
  type CallToolResult,
  type ClientCapabilities,
  type Implementation,
  type Tool as McpToolDefinition,
  type Transport,
} from "@modelcontextprotocol/client";
import { Context, Effect, Layer, Predicate, Schema } from "effect";
import { Tool, Toolkit } from "effect/unstable/ai";

/** Failure establishing or driving the MCP session itself. */
export class McpClientError extends Schema.TaggedError<McpClientError>()("McpClientError", {
  message: Schema.String,
  cause: Schema.Defect(),
}) {}

/** Model-visible failure of one MCP tool call. */
export class McpToolCallFailed extends Schema.TaggedError<McpToolCallFailed>()(
  "McpToolCallFailed",
  {
    tool: Schema.NonEmptyString,
    message: Schema.String,
    cause: Schema.optionalKey(Schema.Defect()),
  },
) {}

/** Model-visible success of one MCP tool call. */
export class McpToolResult extends Schema.Class<McpToolResult>("mcp-client/McpToolResult")({
  content: Schema.Array(Schema.Unknown),
  structuredContent: Schema.optionalKey(Schema.Unknown),
}) {}

/**
 * An Effect AI tool discovered from an MCP server. Parameters stay the server's
 * raw JSON Schema, so the handler receives `unknown` and the server validates.
 */
export type McpTool = Tool.Tool<
  string,
  {
    readonly parameters: typeof Schema.Unknown;
    readonly success: typeof McpToolResult;
    readonly failure: typeof McpToolCallFailed;
    readonly failureMode: "return";
  }
>;

/** Toolkit of every tool advertised by one MCP server. */
export type McpToolkit = Toolkit.Toolkit<Record<string, McpTool>>;

type McpToolHandler = (
  params: Tool.Parameters<McpTool>,
) => Effect.Effect<McpToolResult, McpToolCallFailed>;

export interface McpClientOptions {
  /** Transport created by the caller; ownership transfers to the client session. */
  readonly transport: Transport;
  readonly clientInfo?: Implementation | undefined;
  readonly capabilities?: ClientCapabilities | undefined;
}

export interface McpClientService {
  readonly client: Client;
  readonly tools: ReadonlyArray<McpToolDefinition>;
  readonly toolkit: McpToolkit;
  readonly handlers: Layer.Layer<Tool.HandlersFor<Record<string, McpTool>>>;
}

const defaultClientInfo: Implementation = {
  name: "sandbox-toolkit-mcp-client",
  version: "0.0.0",
};

const argumentsSchema = Schema.Record(Schema.String, Schema.Unknown);

const decodeArguments = Schema.decodeUnknownEffect(argumentsSchema);

const describeCause = (cause: unknown): string =>
  Predicate.isError(cause) ? cause.message : String(cause);

const errorText = (content: CallToolResult["content"]): string =>
  content
    .flatMap((block) => (block.type === "text" ? [block.text] : []))
    .join("\n")
    .slice(0, 4 * 1024);

/**
 * Derives a dynamic Effect AI tool from a discovered MCP tool. The MCP
 * annotations are server-declared hints and stay advisory.
 */
export const toDynamicTool = (tool: McpToolDefinition): McpTool => {
  // MCP servers may advertise JSON Schemas outside a model provider's strict subset.
  let dynamic: McpTool = Tool.dynamic(tool.name, {
    description: tool.description,
    parameters: tool.inputSchema,
    success: McpToolResult,
    failure: McpToolCallFailed,
    failureMode: "return",
  }).annotate(Tool.Strict, false);

  if (tool.annotations !== undefined) {
    dynamic = dynamic
      .annotate(Tool.Readonly, tool.annotations.readOnlyHint ?? false)
      .annotate(Tool.Destructive, tool.annotations.destructiveHint ?? false)
      .annotate(Tool.Idempotent, tool.annotations.idempotentHint ?? false)
      .annotate(Tool.OpenWorld, tool.annotations.openWorldHint ?? false);
  }

  return dynamic;
};

const callTool = Effect.fnUntraced(function* <Arguments>(
  client: Client,
  name: string,
  params: Arguments,
) {
  const args = yield* decodeArguments(params ?? {}).pipe(
    Effect.mapError(
      (cause) =>
        new McpToolCallFailed({
          tool: name,
          message: `Invalid arguments for MCP tool '${name}': ${cause.message}`,
        }),
    ),
  );

  const result = yield* Effect.tryPromise({
    try: (signal) => client.callTool({ name, arguments: args }, { signal }),
    catch: (cause) =>
      new McpToolCallFailed({
        tool: name,
        message: `MCP tool '${name}' failed: ${describeCause(cause)}`,
        cause,
      }),
  });

  if (result.isError === true) {
    return yield* new McpToolCallFailed({ tool: name, message: errorText(result.content) });
  }

  if (result.structuredContent === undefined) {
    return new McpToolResult({ content: result.content });
  }

  return new McpToolResult({
    content: result.content,
    structuredContent: result.structuredContent,
  });
});

const make = Effect.fn("McpClient.make")(function* (options: McpClientOptions) {
  const clientInfo = options.clientInfo ?? defaultClientInfo;

  const client = yield* Effect.acquireRelease(
    Effect.tryPromise({
      try: () => {
        const client = new Client(
          clientInfo,
          options.capabilities === undefined ? undefined : { capabilities: options.capabilities },
        );

        return client.connect(options.transport).then(() => client);
      },
      catch: (cause) =>
        new McpClientError({ message: "Could not connect to the MCP server", cause }),
    }),
    (client) => Effect.promise(() => client.close()).pipe(Effect.ignoreCause),
  );

  const { tools } = yield* Effect.tryPromise({
    try: () => client.listTools(),
    catch: (cause) => new McpClientError({ message: "Could not list MCP tools", cause }),
  });

  const toolkit = Toolkit.make(...tools.map(toDynamicTool));
  const handlers: Record<string, McpToolHandler> = {};

  for (const tool of tools) {
    handlers[tool.name] = (params) => callTool(client, tool.name, params);
  }

  return {
    client,
    tools,
    toolkit,
    handlers: toolkit.toLayer(handlers),
  } satisfies McpClientService;
});

/**
 * A connected MCP server exposing its tools as an Effect AI {@link McpToolkit}.
 * The session lives as long as the layer providing this service.
 */
export class McpClient extends Context.Service<McpClient, McpClientService>()(
  "mcp-client/McpClient",
) {
  static readonly make = make;

  static readonly layer = (options: McpClientOptions): Layer.Layer<McpClient, McpClientError> =>
    Layer.effect(McpClient, make(options));
}
