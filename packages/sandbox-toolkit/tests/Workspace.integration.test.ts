import { execFileSync, spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { createServer, connect } from "node:net";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

import { Effect, Layer } from "effect";
import { afterAll, beforeAll, describe, expect, test } from "vite-plus/test";

import { ApiError, layerFetch } from "../src/internal/client.ts";
import * as Workspace from "../src/Workspace.ts";

/**
 * End-to-end wiring check between this package's `Workspace` client and the Rust
 * workspace module: real HTTP registration, listing, lookup and unregistration.
 * The server is built from the source tree and spawned on a private port with a
 * throwaway root.
 */

const repoRoot = resolve(import.meta.dirname, "../../..");

const serverBinary = join(repoRoot, "target", "debug", "sbxtkt");

const hasCargo = ((): boolean => {
  try {
    execFileSync("cargo", ["--version"], { stdio: "ignore" });

    return true;
  } catch {
    return false;
  }
})();

interface Server {
  readonly baseUrl: string;
  readonly stop: () => Promise<void>;
}

const freePort = (): Promise<number> =>
  new Promise((done, fail) => {
    const probe = createServer();
    probe.once("error", fail);
    probe.listen(0, "127.0.0.1", () => {
      const address = probe.address();

      if (address === null || typeof address === "string") {
        fail(new Error("failed to reserve a port"));

        return;
      }

      const { port } = address;
      probe.close(() => done(port));
    });
  });

const waitForServer = async (port: number): Promise<void> => {
  const deadline = Date.now() + 15_000;

  while (Date.now() < deadline) {
    const connected = await new Promise<boolean>((done) => {
      const socket = connect({ host: "127.0.0.1", port });
      socket.once("connect", () => {
        socket.destroy();
        done(true);
      });
      socket.once("error", () => {
        socket.destroy();
        done(false);
      });
    });

    if (connected) return;
    await new Promise((tick) => setTimeout(tick, 50));
  }

  throw new Error(`the server did not accept connections on port ${port}`);
};

const startServer = async (root: string): Promise<Server> => {
  const port = await freePort();

  const child = spawn(
    serverBinary,
    ["--host", "127.0.0.1", "--port", String(port), "--root", root],
    {
      cwd: repoRoot,
      env: { ...globalThis.process.env, RUST_LOG: "warn" },
      stdio: ["ignore", "ignore", "pipe"],
    },
  );

  let stderr = "";
  child.stderr?.on("data", (chunk: Buffer) => {
    stderr += chunk.toString();
  });

  let started = false;

  const exited = new Promise<never>((_done, fail) => {
    child.once("exit", (code) => {
      if (started) return;
      fail(new Error(`the server exited early with code ${code}: ${stderr}`));
    });
  });

  await Promise.race([waitForServer(port), exited]);
  started = true;

  return {
    baseUrl: `http://127.0.0.1:${port}`,
    stop: async () => {
      if (child.exitCode !== null || child.signalCode !== null) return;
      await new Promise<void>((done) => {
        child.once("exit", () => done());
        child.kill("SIGKILL");
      });
    },
  };
};

let baseUrl = "";

const workspaceLayer = () => Workspace.layer.pipe(Layer.provide(layerFetch({ baseUrl })));

const run = <A, E>(program: Effect.Effect<A, E, Workspace.WorkspaceService>): Promise<A> =>
  Effect.runPromise(Effect.provide(program, workspaceLayer()));

const workspace = Workspace.Workspace;

describe.skipIf(!hasCargo && !existsSync(serverBinary))("Workspace ↔ workspaces", () => {
  let server: Server | undefined;
  let root = "";
  let directory = "";

  beforeAll(async () => {
    root = mkdtempSync(join(tmpdir(), "sbx-workspace-root-"));
    directory = mkdtempSync(join(tmpdir(), "sbx-workspace-dir-"));
    // One skill under the registered root, so listing resolves `skill-docs-demo`.
    mkdirSync(join(directory, ".agents", "skills", "demo"), { recursive: true });
    writeFileSync(
      join(directory, ".agents", "skills", "demo", "SKILL.md"),
      "---\nname: demo\ndescription: A demo skill.\n---\n\nBody.\n",
    );
    server = await startServer(root);
    baseUrl = server.baseUrl;
  }, 600_000);

  afterAll(async () => {
    await server?.stop();
    rmSync(root, { recursive: true, force: true });
    rmSync(directory, { recursive: true, force: true });
  });

  test("registers, resolves and unregisters a workspace", async () => {
    const result = await run(
      Effect.gen(function* () {
        const service = yield* workspace;

        const created = yield* service.create({ id: "docs", root: directory });
        const fetched = yield* service.get("docs");
        yield* service.remove("docs");

        return { created, fetched };
      }),
    );

    expect(result.created).toEqual({ id: "docs", properties: { access: "read-write" } });
    expect(result.fetched).toEqual(result.created);

    const missing = await run(
      Effect.flatMap(workspace, (service) => Effect.flip(service.get("docs"))),
    );

    expect(missing).toBeInstanceOf(Workspace.WorkspaceNotFound);
  });

  test("lists registered workspaces in id order", async () => {
    const ids = await run(
      Effect.gen(function* () {
        const service = yield* workspace;

        yield* service.create({ id: "zeta", root: directory });
        yield* service.create({ id: "alpha", root: directory });

        const handles = yield* service.list();

        yield* Effect.forEach(["zeta", "alpha"], (id) => service.remove(id), { discard: true });

        return handles.map((handle) => handle.id);
      }),
    );

    expect(ids).toContain("alpha");
    expect(ids).toContain("zeta");
    expect([...ids].sort()).toEqual(ids);
  });

  test("echoes the requested access mode", async () => {
    const handle = await run(
      Effect.gen(function* () {
        const service = yield* workspace;

        const created = yield* service.create({
          id: "sealed",
          root: directory,
          access: "read-only",
        });

        yield* service.remove("sealed");

        return created;
      }),
    );

    expect(handle).toEqual({ id: "sealed", properties: { access: "read-only" } });
  });

  test("reports a duplicate id and a bad root distinctly", async () => {
    const duplicate = await run(
      Effect.gen(function* () {
        const service = yield* workspace;

        yield* service.create({ id: "docs", root: directory });

        const error = yield* Effect.flip(service.create({ id: "docs", root: directory }));

        yield* service.remove("docs");

        return error;
      }),
    );

    expect(duplicate).toBeInstanceOf(Workspace.WorkspaceExists);

    const badRoot = await run(
      Effect.flatMap(workspace, (service) =>
        Effect.flip(service.create({ id: "docs", root: "relative/path" })),
      ),
    );

    expect(badRoot).toBeInstanceOf(ApiError);
    expect(badRoot).toMatchObject({ code: "bad_request" });
  });

  test("rejects a malformed id before sending a request", async () => {
    const error = await run(
      Effect.flatMap(workspace, (service) =>
        Effect.flip(service.create({ id: "Bad", root: directory })),
      ),
    );

    expect(error).toBeInstanceOf(Workspace.InvalidWorkspaceId);
  });

  test("reports removing an unknown workspace as not found", async () => {
    const error = await run(
      Effect.flatMap(workspace, (service) => Effect.flip(service.remove("missing"))),
    );

    expect(error).toBeInstanceOf(Workspace.WorkspaceNotFound);
  });

  test("createScoped unregisters the workspace when the scope closes", async () => {
    const registered = await run(
      Effect.scoped(
        Effect.flatMap(workspace, (service) =>
          service.createScoped({ id: "scoped", root: directory }),
        ),
      ),
    );

    expect(registered.id).toBe("scoped");

    const after = await run(
      Effect.flatMap(workspace, (service) => Effect.flip(service.get("scoped"))),
    );

    expect(after).toBeInstanceOf(Workspace.WorkspaceNotFound);
  });

  test("resolves a skill-derived workspace and refuses to remove it", async () => {
    const result = await run(
      Effect.gen(function* () {
        const service = yield* workspace;

        yield* service.create({ id: "docs", root: directory });

        const derived = yield* service.get("skill-docs-demo");
        const error = yield* Effect.flip(service.remove("skill-docs-demo"));

        yield* service.remove("docs");

        return { derived, error };
      }),
    );

    expect(result.derived).toEqual({
      id: "skill-docs-demo",
      properties: { access: "read-only" },
    });
    expect(result.error).toBeInstanceOf(Workspace.ManagedWorkspace);
  });

  test("createScoped tolerates a workspace removed before the scope closes", async () => {
    const id = await run(
      Effect.scoped(
        Effect.gen(function* () {
          const service = yield* workspace;

          const handle = yield* service.createScoped({ id: "scoped", root: directory });
          yield* service.remove("scoped");

          return handle.id;
        }),
      ),
    );

    expect(id).toBe("scoped");
  });
});
