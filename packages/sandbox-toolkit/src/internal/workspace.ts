import { Effect, Match } from "effect";

import {
  InvalidWorkspaceId,
  ManagedWorkspace,
  WorkspaceExists,
  WorkspaceInUse,
  WorkspaceNotFound,
  type WorkspaceError,
} from "../Workspace.ts";
import { ApiError, type ClientError } from "./client.ts";

/** The shared resource-id rules: `[a-z0-9]+(-[a-z0-9]+)*`. */
const workspaceIdPattern = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;

/**
 * Rejects a malformed id before the request, so the failure names the offending
 * id instead of leaving the caller to parse the server's message.
 */
export const validateWorkspaceId = (
  workspace: string,
): Effect.Effect<string, InvalidWorkspaceId> =>
  workspaceIdPattern.test(workspace)
    ? Effect.succeed(workspace)
    : Effect.fail(
        new InvalidWorkspaceId({
          workspace,
          reason: "must match [a-z0-9-] without leading, trailing, or repeated `-`",
        }),
      );

export const collectionUrl = "/workspaces";

export const itemUrl = (workspace: string): string => `/workspaces/${workspace}`;

/** A taken id is a domain conflict; every other transport failure survives. */
export const toCreateError = (workspace: string, error: ClientError): WorkspaceError =>
  error instanceof ApiError && error.code === "conflict"
    ? new WorkspaceExists({ workspace })
    : error;

export const toLookupError = (workspace: string, error: ClientError): WorkspaceError =>
  error instanceof ApiError && error.code === "not_found"
    ? new WorkspaceNotFound({ workspace })
    : error;

/** Unregistering is refused for different reasons, each surfaced distinctly. */
export const toRemoveError = (workspace: string, error: ClientError): WorkspaceError =>
  error instanceof ApiError
    ? Match.value(error.code).pipe(
        Match.when("not_found", () => new WorkspaceNotFound({ workspace })),
        Match.when("workspace_in_use", () => new WorkspaceInUse({ workspace })),
        Match.when("managed_workspace", () => new ManagedWorkspace({ workspace })),
        Match.orElse(() => error),
      )
    : error;
