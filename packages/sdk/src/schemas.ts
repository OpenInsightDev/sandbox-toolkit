import { Schema } from "effect";

import type { DescribeToolParams } from "./generated/DescribeToolParams.ts";
import type { DescribeToolResult } from "./generated/DescribeToolResult.ts";
import type { HealthResult } from "./generated/HealthResult.ts";
import type { ListToolsResult } from "./generated/ListToolsResult.ts";
import type { ReadFileParams } from "./generated/ReadFileParams.ts";
import type { ReadFileResult } from "./generated/ReadFileResult.ts";
import type { TextLine } from "./generated/TextLine.ts";

// `satisfies` (not a `: Schema.Schema<T>` annotation) checks each schema against
// its generated wire type without widening it. `Schema.mutable`/`optionalKey`
// keep the decoded values assignable to those generated types.

export const HealthSchema = Schema.Struct({
  status: Schema.String,
  uptimeSeconds: Schema.Number,
}) satisfies Schema.Schema<HealthResult>;

export const DescribeToolResultSchema = Schema.Struct({
  name: Schema.String,
  path: Schema.String,
}) satisfies Schema.Schema<DescribeToolResult>;

export const ListToolsResultSchema = Schema.Struct({
  tools: Schema.mutable(Schema.Array(DescribeToolResultSchema)),
}) satisfies Schema.Schema<ListToolsResult>;

export const TextLineSchema = Schema.Struct({
  number: Schema.Number,
  text: Schema.String,
}) satisfies Schema.Schema<TextLine>;

export const ReadFileResultSchema = Schema.Struct({
  path: Schema.String,
  lines: Schema.mutable(Schema.Array(TextLineSchema)),
  truncated: Schema.Boolean,
  nextOffset: Schema.optionalKey(Schema.NullOr(Schema.Number)),
}) satisfies Schema.Schema<ReadFileResult>;

export type {
  DescribeToolParams,
  DescribeToolResult,
  HealthResult,
  ListToolsResult,
  ReadFileParams,
  ReadFileResult,
  TextLine,
};
