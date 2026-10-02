import { Result, Schema } from "effect";
import { expect, test } from "vite-plus/test";

import {
  Headers,
  McpConfig,
  MCP_SCHEMA_1_0_0,
  Server,
  StdioEnv,
  StreamableHttpServer,
} from "../src/Mcp.ts";

const decode = <S extends Schema.ConstraintDecoder<unknown>>(schema: S, input: unknown) =>
  Schema.decodeUnknownResult(schema, { onExcessProperty: "error" })(input);

test("decodes a stdio, streamable-http, and sse server by their type tag", () => {
  const decoded = decode(McpConfig, {
    $schema: MCP_SCHEMA_1_0_0,
    mcpServers: {
      cli: {
        type: "stdio",
        command: "./bin/validator",
        args: ["--data", "data/validator"],
        env: { CONFIG: "config.json" },
        cwd: "${PLUGIN_DATA}",
      },
      api: {
        type: "streamable-http",
        url: "https://sandbox.example.com/mcps/api",
        headers: { authorization: "Bearer token" },
      },
      legacy: { type: "sse", url: "https://sandbox.example.com/sse" },
    },
  });

  expect(Result.isSuccess(decoded)).toBe(true);
});

test("requires the fixed $schema identifier", () => {
  const decoded = decode(McpConfig, {
    $schema: "https://agent-plugins.org/schemas/2.0.0/mcp.schema.json",
    mcpServers: {},
  });

  expect(Result.isFailure(decoded)).toBe(true);
});

test("requires a non-empty command for stdio", () => {
  expect(Result.isFailure(decode(Server, { type: "stdio", command: "" }))).toBe(true);
  expect(Result.isFailure(decode(Server, { type: "stdio" }))).toBe(true);
});

test("rejects an unknown transport", () => {
  expect(Result.isFailure(decode(Server, { type: "http", url: "https://x.test" }))).toBe(true);
});

test("rejects an env entry that redeclares a reserved plugin variable", () => {
  expect(Result.isFailure(decode(StdioEnv, { PLUGIN_ROOT: "/elsewhere" }))).toBe(true);
  expect(Result.isFailure(decode(StdioEnv, { PLUGIN_DATA: "/elsewhere" }))).toBe(true);
  expect(Result.isSuccess(decode(StdioEnv, { CONFIG: "config.json" }))).toBe(true);
});

test("rejects a cwd outside the plugin roots", () => {
  expect(Result.isFailure(decode(Server, { type: "stdio", command: "x", cwd: "/abs/path" }))).toBe(
    true,
  );
  expect(Result.isFailure(decode(Server, { type: "stdio", command: "x", cwd: "../out" }))).toBe(
    true,
  );
  expect(Result.isSuccess(decode(Server, { type: "stdio", command: "x", cwd: "./bin" }))).toBe(
    true,
  );
});

test("closes the server object against unmodeled keys", () => {
  expect(
    Result.isFailure(
      decode(StreamableHttpServer, { type: "streamable-http", url: "https://x.test", nope: 1 }),
    ),
  ).toBe(true);
});

test("headers map string keys to string values", () => {
  expect(Result.isSuccess(decode(Headers, { authorization: "Bearer token" }))).toBe(true);
  expect(Result.isFailure(decode(Headers, { authorization: 1 }))).toBe(true);
});
