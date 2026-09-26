import { Record } from "effect";

/** Process routes hang off the workspace prefix in workspace mode, and off the root in direct mode. */
export const route = (workspace: string | undefined, path: string): string =>
  workspace === undefined ? path : `/workspaces/${workspace}${path}`;

/** Named environment variables only; an unset one is omitted rather than sent as null. */
export const targetEnv = (
  env: Record<string, string | undefined> | undefined,
): Record<string, string> =>
  Record.filter(env ?? {}, (value): value is string => value !== undefined);
