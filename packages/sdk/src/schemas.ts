import { Schema } from "effect";

import type { DescribeToolParams } from "./generated/DescribeToolParams.ts";
import type { DescribeToolResult } from "./generated/DescribeToolResult.ts";
import type { ExecParams } from "./generated/ExecParams.ts";
import type { ExecResult } from "./generated/ExecResult.ts";
import type { HealthResult } from "./generated/HealthResult.ts";
import type { ListToolsResult } from "./generated/ListToolsResult.ts";
import type { ReadFileParams } from "./generated/ReadFileParams.ts";
import type { ReadFileResult } from "./generated/ReadFileResult.ts";
import type { TextLine } from "./generated/TextLine.ts";

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

export const ExecResultSchema = Schema.Struct({
  exitCode: Schema.NullOr(Schema.Number),
  stdout: Schema.String,
  stderr: Schema.String,
  truncated: Schema.Boolean,
  outputPath: Schema.optionalKey(Schema.NullOr(Schema.String)),
}) satisfies Schema.Schema<ExecResult>;

export type {
  DescribeToolParams,
  DescribeToolResult,
  ExecParams,
  ExecResult,
  HealthResult,
  ListToolsResult,
  ReadFileParams,
  ReadFileResult,
  TextLine,
};
