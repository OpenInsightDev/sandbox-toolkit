import { Context, Data, Effect, Layer, type Scope } from "effect";
import { HttpClientRequest } from "effect/unstable/http";

import type { CreateWorkspaceRequest } from "./generated/CreateWorkspaceRequest.ts";
import type { WorkspaceAccess } from "./generated/WorkspaceAccess.ts";
import type { WorkspaceHandle } from "./generated/WorkspaceHandle.ts";
import type { WorkspaceList } from "./generated/WorkspaceList.ts";
import { Client, type ClientError } from "./internal/client.ts";
import {
  collectionUrl,
  itemUrl,
  toCreateError,
  toLookupError,
  toRemoveError,
  validateWorkspaceId,
} from "./internal/workspace.ts";

export type { WorkspaceAccess } from "./generated/WorkspaceAccess.ts";

export type { WorkspaceHandle } from "./generated/WorkspaceHandle.ts";

export type { WorkspaceProperties } from "./generated/WorkspaceProperties.ts";

/** A workspace id outside the shared charset: `[a-z0-9]+(-[a-z0-9]+)*`. */
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
 * A registered workspace as the service surfaces it, carrying the wire handle's
 * fields so derived fields can be layered on without touching the protocol.
 */
export interface Workspace extends WorkspaceHandle {}

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

  const create = ((options: CreateWorkspaceOptions) =>
    Effect.gen(function* () {
      const id = yield* validateWorkspaceId(options.id);

      const body: CreateWorkspaceRequest = {
        id,
        root: options.root,
        properties: { access: options.access ?? "read-write" },
      };

      return yield* client
        .json<Workspace>(
          HttpClientRequest.post(collectionUrl).pipe(HttpClientRequest.bodyJsonUnsafe(body)),
        )
        .pipe(Effect.mapError((error) => toCreateError(id, error)));
    })) satisfies WorkspaceService["create"];

  const list = (() =>
    client
      .json<WorkspaceList>(HttpClientRequest.get(collectionUrl))
      .pipe(Effect.map((response) => response.workspaces))) satisfies WorkspaceService["list"];

  const get = ((workspace: string) =>
    Effect.gen(function* () {
      const id = yield* validateWorkspaceId(workspace);

      return yield* client
        .json<Workspace>(HttpClientRequest.get(itemUrl(id)))
        .pipe(Effect.mapError((error) => toLookupError(id, error)));
    })) satisfies WorkspaceService["get"];

  const remove = ((workspace: string) =>
    Effect.gen(function* () {
      const id = yield* validateWorkspaceId(workspace);

      yield* client
        .void(HttpClientRequest.delete(itemUrl(id)))
        .pipe(Effect.mapError((error) => toRemoveError(id, error)));
    })) satisfies WorkspaceService["remove"];

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
export const layer = Layer.effect(Workspace, make());
