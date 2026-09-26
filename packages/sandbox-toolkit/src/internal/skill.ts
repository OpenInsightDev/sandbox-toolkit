import { Effect } from "effect";

import type { SkillMetadata as SkillMetadataResponse } from "../generated/SkillMetadata.ts";
import {
  InvalidSkillId,
  SkillNotFound,
  type ListSkillsOptions,
  type SkillError,
  type SkillMetadata,
} from "../Skill.ts";
import { ApiError, type ClientError } from "./client.ts";
import { route } from "./prelude.ts";

/** The shared id rules: `[a-z0-9]+(-[a-z0-9]+)*`, at most 64 characters. */
const skillIdPattern = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;

const maxSkillIdLength = 64;

export const validateSkillId = (skillId: string): Effect.Effect<string, InvalidSkillId> =>
  skillId.length <= maxSkillIdLength && skillIdPattern.test(skillId)
    ? Effect.succeed(skillId)
    : Effect.fail(
        new InvalidSkillId({
          skillId,
          reason:
            "must match [a-z0-9-] up to 64 characters, without leading, trailing, or repeated `-`",
        }),
      );

/** The collection URL, with only the paging options that were given. */
export const listUrl = (workspace: string | undefined, options?: ListSkillsOptions): string => {
  const base = route(workspace, "/skills");
  const params = new URLSearchParams();

  if (options?.offset !== undefined) params.set("offset", `${options.offset}`);

  if (options?.limit !== undefined) params.set("limit", `${options.limit}`);

  const query = params.toString();

  return query === "" ? base : `${base}?${query}`;
};

export const itemUrl = (workspace: string | undefined, skillId: string): string =>
  `${route(workspace, "/skills")}/${skillId}`;

/** The optional frontmatter fields, populated so an absent one stays absent. */
type OptionalMetadata = {
  license?: string;
  compatibility?: string;
  metadata?: Readonly<Record<string, string>>;
};

export const toMetadata = (raw: SkillMetadataResponse): SkillMetadata => {
  const optional: OptionalMetadata = {};

  if (raw.license !== undefined) optional.license = raw.license;

  if (raw.compatibility !== undefined) optional.compatibility = raw.compatibility;

  if (raw.metadata !== undefined) optional.metadata = raw.metadata;

  return {
    id: raw.id,
    root: raw.root,
    name: raw.name,
    description: raw.description,
    uri: raw.uri,
    workspace: raw.workspace_id,
    ...optional,
  };
};

/** A missing skill is a domain error; every other transport failure survives. */
export const toReadError = (skillId: string, error: ClientError): SkillError =>
  error instanceof ApiError && error.code === "not_found" ? new SkillNotFound({ skillId }) : error;
