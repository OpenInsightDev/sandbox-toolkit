import { Schema } from "effect";

import type { CopyParams } from "./generated/CopyParams.ts";
import type { CopyResult } from "./generated/CopyResult.ts";
import type { DescribeToolParams } from "./generated/DescribeToolParams.ts";
import type { DescribeToolResult } from "./generated/DescribeToolResult.ts";
import type { ExecParams } from "./generated/ExecParams.ts";
import type { ExecResult } from "./generated/ExecResult.ts";
import type { HealthResult } from "./generated/HealthResult.ts";
import type { ListParams } from "./generated/ListParams.ts";
import type { ListResult } from "./generated/ListResult.ts";
import type { ListToolsResult } from "./generated/ListToolsResult.ts";
import type { MkdirParams } from "./generated/MkdirParams.ts";
import type { MkdirResult } from "./generated/MkdirResult.ts";
import type { MoveParams } from "./generated/MoveParams.ts";
import type { MoveResult } from "./generated/MoveResult.ts";
import type { ReadFileParams } from "./generated/ReadFileParams.ts";
import type { ReadFileResult } from "./generated/ReadFileResult.ts";
import type { RemoveParams } from "./generated/RemoveParams.ts";
import type { RemoveResult } from "./generated/RemoveResult.ts";
import type { Resource } from "./generated/Resource.ts";
import type { ResourceKind } from "./generated/ResourceKind.ts";
import type { ShellParams } from "./generated/ShellParams.ts";
import type { StatParams } from "./generated/StatParams.ts";
import type { StatResult } from "./generated/StatResult.ts";
import type { TextLine } from "./generated/TextLine.ts";
import type { WriteFileParams } from "./generated/WriteFileParams.ts";
import type { WriteFileResult } from "./generated/WriteFileResult.ts";

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

export const ResourceKindSchema = Schema.Literals([
  "file",
  "directory",
  "symlink",
]) satisfies Schema.Schema<ResourceKind>;

export const ResourceSchema = Schema.Struct({
  path: Schema.String,
  name: Schema.String,
  kind: ResourceKindSchema,
  size: Schema.Number,
  modifiedAt: Schema.optionalKey(Schema.NullOr(Schema.Number)),
  createdAt: Schema.optionalKey(Schema.NullOr(Schema.Number)),
  readOnly: Schema.Boolean,
  target: Schema.optionalKey(Schema.NullOr(Schema.String)),
}) satisfies Schema.Schema<Resource>;

export const StatResultSchema = Schema.Struct({
  resource: ResourceSchema,
}) satisfies Schema.Schema<StatResult>;

export const ListResultSchema = Schema.Struct({
  path: Schema.String,
  entries: Schema.mutable(Schema.Array(ResourceSchema)),
}) satisfies Schema.Schema<ListResult>;

export const MkdirResultSchema = Schema.Struct({
  resource: ResourceSchema,
}) satisfies Schema.Schema<MkdirResult>;

export const WriteFileResultSchema = Schema.Struct({
  resource: ResourceSchema,
}) satisfies Schema.Schema<WriteFileResult>;

export const RemoveResultSchema = Schema.Struct({
  path: Schema.String,
}) satisfies Schema.Schema<RemoveResult>;

export const CopyResultSchema = Schema.Struct({
  resource: ResourceSchema,
}) satisfies Schema.Schema<CopyResult>;

export const MoveResultSchema = Schema.Struct({
  resource: ResourceSchema,
}) satisfies Schema.Schema<MoveResult>;

export const ExecResultSchema = Schema.Struct({
  exitCode: Schema.NullOr(Schema.Number),
  stdout: Schema.String,
  stderr: Schema.String,
  truncated: Schema.Boolean,
  outputPath: Schema.optionalKey(Schema.NullOr(Schema.String)),
}) satisfies Schema.Schema<ExecResult>;

export type {
  CopyParams,
  CopyResult,
  DescribeToolParams,
  DescribeToolResult,
  ExecParams,
  ExecResult,
  HealthResult,
  ListParams,
  ListResult,
  ListToolsResult,
  MkdirParams,
  MkdirResult,
  MoveParams,
  MoveResult,
  ReadFileParams,
  ReadFileResult,
  RemoveParams,
  RemoveResult,
  Resource,
  ResourceKind,
  ShellParams,
  StatParams,
  StatResult,
  TextLine,
  WriteFileParams,
  WriteFileResult,
};
