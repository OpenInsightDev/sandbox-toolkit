/** Process routes hang off the workspace prefix in workspace mode, and off the root in direct mode. */
export const route = (workspace: string | undefined, path: string): string =>
  workspace === undefined ? path : `/workspaces/${workspace}${path}`;
