import { Context, Data, Effect, Layer, type Scope } from "effect";
import { HttpClientRequest } from "effect/unstable/http";

import type { CreateWorkspaceRequest } from "./generated/CreateWorkspaceRequest.ts";
import type { Metadata } from "./generated/Metadata.ts";
import type { WorkspaceAccess } from "./generated/WorkspaceAccess.ts";
import type { WorkspaceList } from "./generated/WorkspaceList.ts";
import { FileSystem, make as makeFs } from "./FileSystem.ts";
import { Client, type ClientError, layer as clientLayer } from "./internal/client.ts";
import { layer as http2WebSocket } from "./internal/Http2WebSocket.ts";
import { TUSClient, make as makeTus } from "./internal/TUSClient.ts";
import {
  collectionUrl,
  itemUrl,
  toCreateError,
  toLookupError,
  toRemoveError,
  validateWorkspaceId,
  validateWorkspaceRef,
} from "./internal/workspace.ts";
import { Mcp, make as makeMcp } from "./Mcp.ts";
import { Plugin, make as makePlugin } from "./Plugin.ts";
import { Process, make as makeProcess } from "./Process.ts";
import { Skill, make as makeSkill } from "./Skill.ts";
import {
  Terminal,
  type TerminalError,
  type TerminalOptions,
  make as makeTerminal,
} from "./Terminal.ts";

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
 * A registered workspace as the service surfaces it: the wire metadata plus the
 * features bound to the workspace prefix the id addresses.
 */
export interface Workspace extends Metadata {
  /** Files and directories, addressed relative to the workspace root. */
  readonly fs: FileSystem;
  readonly mcp: Mcp;
  readonly plugin: Plugin;
  /** Commands and processes, with `cwd` relative to the workspace root. */
  readonly process: Process["Service"];
  readonly skill: Skill;
  /**
   * Opens a pty session in the workspace; the session lives as long as the
   * surrounding scope.
   */
  readonly terminal: (
    options?: TerminalOptions,
  ) => Effect.Effect<Terminal, TerminalError, Scope.Scope>;
}

export interface WorkspaceService {
  /**
   * Register a remote absolute directory under an id and return the workspace.
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

  /** One registered workspace, with the services its mounts expose. */
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
  const tus = yield* makeTus().pipe(Effect.provideService(Client, client));

  // The features a workspace prefixes are built against the client the service
  // was layered with, so callers get them without providing `Client` again.
  const bind = Effect.fn("Workspace.bind")(function* (metadata: Metadata) {
    const id = metadata.id;

    const fs = yield* makeFs({ workspace: id }).pipe(
      Effect.provideService(Client, client),
      Effect.provideService(TUSClient, tus),
    );

    const mcp = yield* makeMcp({ workspace: id }).pipe(Effect.provideService(Client, client));

    const plugin = yield* makePlugin({ workspace: id }).pipe(Effect.provideService(Client, client));

    const process = yield* makeProcess({ workspace: id }).pipe(
      Effect.provideService(Client, client),
    );

    const skill = yield* makeSkill({ workspace: id }).pipe(Effect.provideService(Client, client));

    const terminal = (options?: TerminalOptions) =>
      makeTerminal({ workspace: id, ...options }).pipe(
        Effect.provideService(Client, client),
        Effect.provide(http2WebSocket),
      );

    return { ...metadata, fs, mcp, plugin, process, skill, terminal } satisfies Workspace;
  });

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
        Effect.flatMap(bind),
        Effect.mapError((error) => toCreateError(id, error)),
      );
  }) satisfies WorkspaceService["create"];

  const list = (() =>
    client
      .json<WorkspaceList>(HttpClientRequest.get(collectionUrl))
      .pipe(
        Effect.flatMap((workspaces) => Effect.forEach(workspaces, bind)),
      )) satisfies WorkspaceService["list"];

  const get = Effect.fn("Workspace.get")(function* (workspace: string) {
    const id = yield* validateWorkspaceRef(workspace);

    return yield* client.json<Metadata>(HttpClientRequest.get(itemUrl(id))).pipe(
      Effect.flatMap(bind),
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
    Effect.acquireRelease(create(options), (workspace) =>
      remove(workspace.id).pipe(
        Effect.catchTag("WorkspaceNotFound", () => Effect.void),
        Effect.orDie,
      ),
    )) satisfies WorkspaceService["createScoped"];

  return Workspace.of({ create, createScoped, list, get, remove });
});

/** The workspace control plane. */
export const layer = (config: { readonly baseUrl?: string | URL | undefined } = {}) =>
  Layer.effect(Workspace, make()).pipe(Layer.provide(clientLayer(config)));
