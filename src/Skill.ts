import { Context, Data, Effect, Layer } from "effect";

/**
 * A skill id outside the Agent Skills charset: `[a-z0-9-]`, at most 64
 * characters, with no leading or trailing `-` and no `--`.
 */
export class InvalidSkillId extends Data.TaggedError("InvalidSkillId")<{
  readonly skillId: string;
  readonly reason: string;
}> {}

/** No skill is discovered under the id at the mount point. */
export class SkillNotFound extends Data.TaggedError("SkillNotFound")<{
  readonly skillId: string;
}> {}

export type SkillError = InvalidSkillId | SkillNotFound;

/**
 * The metadata layer of a skill's progressive disclosure: its `SKILL.md`
 * frontmatter plus the addressing the service derives, without the body.
 */
export interface SkillMetadata {
  /** Directory name, which must equal the frontmatter `name`. */
  readonly id: string;
  /** Discovered skill directory, and the root of the derived workspace. */
  readonly root: string;
  readonly name: string;
  readonly description: string;
  readonly license?: string;
  readonly compatibility?: string;
  readonly metadata?: Readonly<Record<string, string>>;
  /** Address of the skill's body at this mount point. */
  readonly uri: string;
  /**
   * Id of the read-only workspace holding the skill's other files, opened with
   * `FileSystem.layerForWorkspace`.
   */
  readonly workspace: string;
}

export interface SkillList {
  readonly skills: ReadonlyArray<SkillMetadata>;
}

export interface ListSkillsOptions {
  /** Zero-based index of the first skill to return, in discovery order. */
  readonly offset?: number;
  /** Maximum number of skills to return. */
  readonly limit?: number;
}

export interface Skill {
  /**
   * Metadata of every skill discovered at the mount point.
   *
   * The mount point is rescanned per call, so directory changes are visible
   * immediately, and subdirectories without a valid `SKILL.md` are skipped
   * instead of reported.
   */
  readonly list: (options?: ListSkillsOptions) => Effect.Effect<SkillList, SkillError>;

  /**
   * The body of the skill's `SKILL.md` with the frontmatter removed; the
   * frontmatter fields are carried by {@link SkillMetadata}.
   */
  readonly read: (skillId: string) => Effect.Effect<string, SkillError>;
}

export const Skill: Context.Service<Skill, Skill> = Context.Service("skill");

/**
 * The skill service over the mount point of a workspace, discovered under its
 * `.agents/skills` directory.
 */
export const layerForWorkspace = ({ workspace: _workspace }: { workspace: string }) =>
  Layer.effect(Skill, Effect.die(new Error("not implemented")));

/** The skill service over the global mount point. */
export const layer = Layer.effect(Skill, Effect.die(new Error("not implemented")));
