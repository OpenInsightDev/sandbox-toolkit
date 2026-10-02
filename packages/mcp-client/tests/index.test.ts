import { Tool } from "effect/unstable/ai";
import { expect, test } from "vite-plus/test";
import { toDynamicTool } from "../src/index.ts";

test("maps a discovered MCP tool to a dynamic Effect AI tool", () => {
  const tool = toDynamicTool({
    name: "search",
    description: "Search the docs",
    inputSchema: {
      type: "object",
      properties: { query: { type: "string" } },
      required: ["query"],
    },
  });

  expect(tool.name).toBe("search");
  expect(tool.description).toBe("Search the docs");
  expect(Tool.isDynamic(tool)).toBe(true);
  expect(Tool.getJsonSchema(tool)).toEqual({
    type: "object",
    properties: { query: { type: "string" } },
    required: ["query"],
  });
});
