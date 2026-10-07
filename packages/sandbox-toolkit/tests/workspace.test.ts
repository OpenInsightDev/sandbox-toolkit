import { Effect } from "effect";
import { expect, test } from "vite-plus/test";
import { Workspace } from "../src/Workspace.ts";
import { startSandbox, workspaceLayer } from "./harness.ts";

test("lists the preset global workspace the server starts with", async () => {
  const sandbox = await startSandbox();

  try {
    const program = Effect.gen(function* () {
      const workspaces = yield* Workspace;
      return yield* workspaces.list();
    });

    const listed = await Effect.runPromise(
      Effect.provide(program, workspaceLayer(sandbox.baseUrl)),
    );

    expect(listed.map((workspace) => workspace.id)).toContain("global");
  } finally {
    sandbox.stop();
  }
});
