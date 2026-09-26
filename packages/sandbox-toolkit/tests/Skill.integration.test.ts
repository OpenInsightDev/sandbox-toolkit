import { execFileSync, spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { createServer, connect } from "node:net";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

import { Effect, Layer } from "effect";
import { afterAll, beforeAll, describe, expect, test } from "vite-plus/test";

import { ApiError, layerFetch } from "../src/internal/client.ts";
import * as Skill from "../src/Skill.ts";

/**
 * End-to-end wiring check between this package's `Skill` client and the Rust
 * skill module: real HTTP, real SKILL.md discovery under both mounts. The server
 * is built from the source tree and spawned with a throwaway root and `HOME`, so
 * the global `.agents/skills` and the workspace-registered one are both
 * controlled by the test.
 */

const repoRoot = resolve(import.meta.dirname, "../../..");

const manifest = "crates/sandbox-toolkit/Cargo.toml";

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

const startServer = async (root: string, home: string): Promise<Server> => {
  const port = await freePort();

  const child = spawn(
    serverBinary,
    ["--host", "127.0.0.1", "--port", String(port), "--root", root],
    {
      cwd: repoRoot,
      env: { ...globalThis.process.env, HOME: home, RUST_LOG: "warn" },
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

const registerWorkspace = async (server: Server, id: string, root: string): Promise<void> => {
  const response = await fetch(`${server.baseUrl}/workspaces`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ id, root }),
  });

  if (response.status !== 201) {
    throw new Error(`failed to register workspace: ${response.status} ${await response.text()}`);
  }
};

const writeSkill = (skillsDir: string, id: string, frontmatter: string, body: string): void => {
  const dir = join(skillsDir, id);

  mkdirSync(dir, { recursive: true });
  writeFileSync(join(dir, "SKILL.md"), `---\n${frontmatter}\n---\n\n${body}`);
};

let baseUrl = "";

const skillLayer = (workspace?: string) =>
  (workspace === undefined ? Skill.layer : Skill.layerForWorkspace({ workspace })).pipe(
    Layer.provide(layerFetch({ baseUrl })),
  );

const runSkill = <A, E>(
  program: Effect.Effect<A, E, Skill.Skill>,
  workspace?: string,
): Promise<A> => Effect.runPromise(Effect.provide(program, skillLayer(workspace)));

describe.skipIf(!hasCargo && !existsSync(serverBinary))("Skill ↔ skills", () => {
  let server: Server | undefined;
  let root = "";
  let home = "";
  let workspaceRoot = "";
  let globalSkills = "";
  let workspaceSkills = "";

  beforeAll(async () => {
    if (hasCargo) {
      execFileSync("cargo", ["build", "--manifest-path", manifest], {
        cwd: repoRoot,
        stdio: "inherit",
      });
    }

    root = mkdtempSync(join(tmpdir(), "sbx-skill-root-"));
    home = mkdtempSync(join(tmpdir(), "sbx-skill-home-"));
    workspaceRoot = mkdtempSync(join(tmpdir(), "sbx-skill-ws-"));
    globalSkills = join(home, ".agents", "skills");
    workspaceSkills = join(workspaceRoot, ".agents", "skills");

    writeSkill(
      globalSkills,
      "deploy",
      "name: deploy\ndescription: Roll out a service.\nlicense: MIT\ncompatibility: Requires kubectl\nmetadata:\n  author: acme",
      "Ship it.\n",
    );
    writeSkill(globalSkills, "other", "name: other\ndescription: Another skill.", "Other.");
    writeFileSync(join(globalSkills, "deploy", "run.sh"), "#!/bin/sh\necho hi\n");
    // A directory without a valid SKILL.md is skipped.
    mkdirSync(join(globalSkills, "empty"));
    writeSkill(globalSkills, "mismatch", "name: different\ndescription: No.", "No.");
    writeFileSync(join(globalSkills, "loose.txt"), "not a skill");

    writeSkill(
      workspaceSkills,
      "deploy",
      "name: deploy\ndescription: Workspace deploy.",
      "Workspace body.\n",
    );

    server = await startServer(root, home);
    baseUrl = server.baseUrl;
    await registerWorkspace(server, "docs", workspaceRoot);
  }, 600_000);

  afterAll(async () => {
    await server?.stop();
    rmSync(root, { recursive: true, force: true });
    rmSync(home, { recursive: true, force: true });
    rmSync(workspaceRoot, { recursive: true, force: true });
  });

  test("lists the global mount and skips invalid children", async () => {
    const list = await runSkill(Effect.flatMap(Skill.Skill, (skill) => skill.list()));

    expect(list.skills.map((skill) => skill.id)).toEqual(["deploy", "other"]);

    const deploy = list.skills[0];

    expect(deploy).toMatchObject({
      id: "deploy",
      name: "deploy",
      description: "Roll out a service.",
      license: "MIT",
      compatibility: "Requires kubectl",
      metadata: { author: "acme" },
      uri: "/skills/deploy",
      workspace: "skill-deploy",
    });
    expect(deploy?.root).toBe(realpathSync(join(globalSkills, "deploy")));

    // Optional fields an author omitted stay absent.
    expect(list.skills[1]).not.toHaveProperty("license");
    expect(list.skills[1]).not.toHaveProperty("metadata");
  });

  test("reads the global body without the frontmatter", async () => {
    const body = await runSkill(Effect.flatMap(Skill.Skill, (skill) => skill.read("deploy")));

    expect(body).toBe("Ship it.\n");
  });

  test("paginates the listing", async () => {
    const list = await runSkill(
      Effect.flatMap(Skill.Skill, (skill) => skill.list({ offset: 1, limit: 1 })),
    );

    expect(list.skills.map((skill) => skill.id)).toEqual(["other"]);
  });

  test("rescans discovery on every call", async () => {
    writeSkill(globalSkills, "gamma", "name: gamma\ndescription: Added later.", "Late.");

    const list = await runSkill(Effect.flatMap(Skill.Skill, (skill) => skill.list()));

    expect(list.skills.map((skill) => skill.id)).toEqual(["deploy", "gamma", "other"]);
  });

  test("lists and reads under a workspace mount", async () => {
    const list = await runSkill(
      Effect.flatMap(Skill.Skill, (skill) => skill.list()),
      "docs",
    );

    expect(list.skills[0]).toMatchObject({
      id: "deploy",
      description: "Workspace deploy.",
      uri: "/workspaces/docs/skills/deploy",
      workspace: "skill-docs-deploy",
    });

    const body = await runSkill(
      Effect.flatMap(Skill.Skill, (skill) => skill.read("deploy")),
      "docs",
    );

    expect(body).toBe("Workspace body.\n");
  });

  test("serves a skill's bundled files through its derived workspace", async () => {
    const list = await runSkill(Effect.flatMap(Skill.Skill, (skill) => skill.list()));

    const deploy = list.skills.find((skill) => skill.id === "deploy");

    expect(deploy).toBeDefined();

    // The derived workspace is exercised through the file API directly, since the
    // `FileSystem` client's raw read is not wired to this server yet.
    const response = await fetch(`${baseUrl}/workspaces/${deploy?.workspace}/fs?type=content`, {
      method: "QUERY",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ path: "run.sh" }),
    });

    expect(response.status).toBe(200);
    expect(await response.json()).toMatchObject({ content: "#!/bin/sh\necho hi\n" });
  });

  test("rejects a malformed id and an unknown one distinctly", async () => {
    const malformed = await runSkill(
      Effect.flatMap(Skill.Skill, (skill) => Effect.flip(skill.read("Bad"))),
    );
    const missing = await runSkill(
      Effect.flatMap(Skill.Skill, (skill) => Effect.flip(skill.read("missing"))),
    );

    expect(malformed).toBeInstanceOf(Skill.InvalidSkillId);
    expect(missing).toBeInstanceOf(Skill.SkillNotFound);
  });

  test("reports an unknown workspace as an API error", async () => {
    const error = await runSkill(
      Effect.flatMap(Skill.Skill, (skill) => Effect.flip(skill.list())),
      "missing",
    );

    expect(error).toBeInstanceOf(ApiError);
    expect(error).toMatchObject({ code: "not_found", status: 404 });
  });
});
