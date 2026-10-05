import { Context, Data, Effect, Layer, type Scope } from "effect";
import { HttpClientRequest } from "effect/unstable/http";

import type { CreateWorkspaceRequest } from "./generated/CreateWorkspaceRequest.ts";
import type { Metadata } from "./generated/Metadata.ts";
import type { WorkspaceAccess } from "./generated/WorkspaceAccess.ts";
import type { WorkspaceList } from "./generated/WorkspaceList.ts";
import { Client, layer as clientLayer, type ClientError } from "./internal/client.ts";
import {
  collectionUrl,
  itemUrl,
  toCreateError,
  toLookupError,
  toRemoveError,
  validateWorkspaceId,
  validateWorkspaceRef,
} from "./internal/workspace.ts";
import { make as makeSkill, type Skill } from "./Skill.ts";

/** A reference outside the id charset: registered `[a-z0-9-]`, derived with `.`. */
export class InvalidWorkspaceId extends Data.TaggedError("InvalidWorkspaceId")<{
  readonly workspace: string;
  readonly reason: string;
}> {}

/** No workspace is registered under the id. */
export class WorkspaceNotFound extends Data.TaggedError("WorkspaceNotFound")<{
  readonly workspace: string;
}> {}

/** The id is already taken by another registration. */
export class WorkspaceExists extends Data.TaggedError("WorkspaceExists")<{
  readonly workspace: string;
}> {}

/** The workspace is still referenced by another resource and cannot be removed. */
export class WorkspaceInUse extends Data.TaggedError("WorkspaceInUse")<{
  readonly workspace: string;
}> {}

/** The workspace is derived from another resource and is not directly removable. */
export class ManagedWorkspace extends Data.TaggedError("ManagedWorkspace")<{
  readonly workspace: string;
}> {}

export type WorkspaceError =
  | ClientError
  | InvalidWorkspaceId
  | WorkspaceNotFound
  | WorkspaceExists
  | WorkspaceInUse
  | ManagedWorkspace;

export interface CreateWorkspaceOptions {
  /** Unique id, also the URL path segment other resources derive their own from. */
  readonly id: string;
  /** Remote absolute path to register as the workspace root. */
  readonly root: string;
  /** Fixed at registration; defaults to `"read-write"`. */
  readonly access?: WorkspaceAccess | undefined;
}

/**
 * A registered workspace as the service surfaces it, carrying the wire
 * metadata plus the services bound to the workspace's own mount points.
 */
export interface Workspace extends Metadata {
  /** The skill service over this workspace's `/skills` mount. */
  readonly skill: Skill;
}

export interface WorkspaceService {
  /**
   * Register a remote absolute directory under an id and return its handle.
   *
   * **Details**
   *
   * A malformed id fails before the request. The root is validated by the
   * server, which requires an existing directory.
   */
  readonly create: (options: CreateWorkspaceOptions) => Effect.Effect<Workspace, WorkspaceError>;

  /**
   * Register a workspace and unregister it when the surrounding scope closes.
   *
   * **Details**
   *
   * Cleanup ignores a workspace that is already gone and surfaces any other
   * failure as a defect, so a scoped registration does not outlive its scope.
   */
  readonly createScoped: (
    options: CreateWorkspaceOptions,
  ) => Effect.Effect<Workspace, WorkspaceError, Scope.Scope>;

  /** Every registered workspace, in id order. */
  readonly list: () => Effect.Effect<ReadonlyArray<Workspace>, WorkspaceError>;

  /** The handle of one registered workspace. */
  readonly get: (workspace: string) => Effect.Effect<Workspace, WorkspaceError>;

  /**
   * Unregister a workspace.
   *
   * **Details**
   *
   * Only the registration is lifted; the remote directory itself is untouched.
   */
  readonly remove: (workspace: string) => Effect.Effect<void, WorkspaceError>;
}

export const Workspace: Context.Service<WorkspaceService, WorkspaceService> =
  Context.Service("workspace");

export const make = Effect.fn("Workspace.make")(function* () {
  const client = yield* Client;

  const withSkill = (metadata: Metadata): Effect.Effect<Workspace> =>
    makeSkill({ workspace: metadata.id }).pipe(
      Effect.provideService(Client, client),
      Effect.map((skill) => ({ ...metadata, skill })),
    );

  const create = Effect.fn("Workspace.create")(function* (options: CreateWorkspaceOptions) {
    const id = yield* validateWorkspaceId(options.id);

    const body: CreateWorkspaceRequest = {
      id,
      root: options.root,
      access: options.access ?? "read-write",
    };

    return yield* client
      .json<Metadata>(
        HttpClientRequest.post(collectionUrl).pipe(HttpClientRequest.bodyJsonUnsafe(body)),
      )
      .pipe(
        Effect.flatMap(withSkill),
        Effect.mapError((error) => toCreateError(id, error)),
      );
  }) satisfies WorkspaceService["create"];

  const list = (() =>
    client
      .json<WorkspaceList>(HttpClientRequest.get(collectionUrl))
      .pipe(
        Effect.flatMap((workspaces) => Effect.forEach(workspaces, withSkill)),
      )) satisfies WorkspaceService["list"];

  const get = Effect.fn("Workspace.get")(function* (workspace: string) {
    const id = yield* validateWorkspaceRef(workspace);

    return yield* client.json<Metadata>(HttpClientRequest.get(itemUrl(id))).pipe(
      Effect.flatMap(withSkill),
      Effect.mapError((error) => toLookupError(id, error)),
    );
  }) satisfies WorkspaceService["get"];

  const remove = Effect.fn("Workspace.remove")(function* (workspace: string) {
    const id = yield* validateWorkspaceRef(workspace);

    yield* client
      .void(HttpClientRequest.delete(itemUrl(id)))
      .pipe(Effect.mapError((error) => toRemoveError(id, error)));
  }) satisfies WorkspaceService["remove"];

  const createScoped = ((options: CreateWorkspaceOptions) =>
    Effect.acquireRelease(create(options), (handle) =>
      remove(handle.id).pipe(
        Effect.catchTag("WorkspaceNotFound", () => Effect.void),
        Effect.orDie,
      ),
    )) satisfies WorkspaceService["createScoped"];

  return Workspace.of({ create, createScoped, list, get, remove });
});

/** The workspace control plane. */
export const layer = (config: { readonly baseUrl?: string | URL | undefined } = {}) =>
  Layer.effect(Workspace, make()).pipe(Layer.provide(clientLayer(config)));
