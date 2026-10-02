import { execFileSync, spawn } from "node:child_process";
import {
  appendFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  realpathSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { createServer, connect } from "node:net";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

import { Duration, Effect, Layer, Option, Stream } from "effect";
import type { WatchEvent } from "effect/FileSystem";
import { FetchHttpClient } from "effect/unstable/http";
import { afterAll, beforeAll, describe, expect, test } from "vite-plus/test";

import * as FileSystem from "../src/FileSystem.ts";
import * as Workspace from "../src/Workspace.ts";

/**
 * End-to-end wiring check between this package's `FileSystem` client and the
 * Rust resource server: real HTTP, real files, real glob and directory walks.
 * The server is built from the source tree and spawned on a private port with a
 * throwaway root and a registered workspace.
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

let baseUrl = "";

/** The handle a workspace registered with the default access mode carries. */
const workspaceHandle = (id: string): Workspace.WorkspaceHandle => ({
  id,
  properties: { access: "read-write" },
});

const fileSystemLayer = (workspace?: string) =>
  (workspace === undefined
    ? FileSystem.layer({ baseUrl })
    : FileSystem.layerForWorkspace({ workspace: workspaceHandle(workspace), baseUrl })
  ).pipe(Layer.provide(FetchHttpClient.layer));

const run = <A, E>(
  program: Effect.Effect<A, E, FileSystem.FileSystem>,
  workspace?: string,
): Promise<A> => Effect.runPromise(Effect.provide(program, fileSystemLayer(workspace)));

/** Flip a program that is expected to fail and hand back its platform error. */
const failure = (
  body: (fs: FileSystem.FileSystem) => Effect.Effect<unknown, FileSystem.FileSystemError>,
  workspace?: string,
): Promise<FileSystem.FileSystemError> =>
  run(
    Effect.flatMap(FileSystem.FileSystem, (fs) => Effect.flip(body(fs))),
    workspace,
  );

const sorted = (values: ReadonlyArray<string>): Array<string> => [...values].sort();

/**
 * Subscribes to `watchPath`, then runs `change` once the subscription has had
 * time to register, and resolves the first event matching `predicate`. The
 * delay covers only watch setup; a change made before the server starts
 * reporting would otherwise be lost.
 */
const watchOnce = (
  watchPath: string,
  predicate: (event: WatchEvent) => boolean,
  change: Effect.Effect<void>,
  workspace?: string,
  options?: { readonly recursive?: boolean | undefined },
): Promise<WatchEvent> =>
  run(
    Effect.gen(function* () {
      const fs = yield* FileSystem.FileSystem;

      const [events] = yield* Effect.all(
        [
          fs
            .watch(watchPath, options)
            .pipe(
              Stream.filter(predicate),
              Stream.take(1),
              Stream.runCollect,
              Effect.timeout(Duration.seconds(10)),
            ),
          Effect.gen(function* () {
            yield* Effect.sleep(Duration.millis(300));
            yield* change;
          }),
        ],
        { concurrency: 2 },
      );

      return Array.from(events)[0]!;
    }),
    workspace,
  );

describe.skipIf(!hasCargo && !existsSync(serverBinary))("FileSystem ↔ fs", () => {
  let server: Server | undefined;
  let root = "";
  let workspaceRoot = "";
  let directRoot = "";
  let watchRoot = "";
  let watchDirectRoot = "";
  let linesRoot = "";

  beforeAll(async () => {
    root = mkdtempSync(join(tmpdir(), "sbx-fs-root-"));
    workspaceRoot = mkdtempSync(join(tmpdir(), "sbx-fs-ws-"));
    directRoot = mkdtempSync(join(tmpdir(), "sbx-fs-direct-"));
    watchRoot = mkdtempSync(join(tmpdir(), "sbx-fs-watch-"));
    watchDirectRoot = realpathSync(mkdtempSync(join(tmpdir(), "sbx-fs-watch-direct-")));
    linesRoot = mkdtempSync(join(tmpdir(), "sbx-fs-lines-"));

    writeFileSync(join(workspaceRoot, "notes.txt"), "hello\nworld\n");
    writeFileSync(join(workspaceRoot, "readme.md"), "line one\nline two");
    mkdirSync(join(workspaceRoot, "sub"));
    writeFileSync(join(workspaceRoot, "sub", "deep.txt"), "deep");
    // A link whose target leaves the workspace, to exercise confinement.
    symlinkSync("..", join(workspaceRoot, "escape"));

    // More lines than one `QUERY ?type=lines` page, to force pagination.
    writeFileSync(
      join(linesRoot, "many.txt"),
      Array.from({ length: 1500 }, (_, index) => `line ${index}`).join("\n"),
    );

    writeFileSync(join(directRoot, "hello.txt"), "hello");
    mkdirSync(join(directRoot, "nested"));
    writeFileSync(join(directRoot, "nested", "deep.txt"), "deep");

    server = await startServer(root);
    baseUrl = server.baseUrl;
    await registerWorkspace(server, "docs", workspaceRoot);
    await registerWorkspace(server, "watch", watchRoot);
    await registerWorkspace(server, "lines", linesRoot);
  }, 600_000);

  afterAll(async () => {
    await server?.stop();
    rmSync(root, { recursive: true, force: true });
    rmSync(workspaceRoot, { recursive: true, force: true });
    rmSync(directRoot, { recursive: true, force: true });
    rmSync(watchRoot, { recursive: true, force: true });
    rmSync(watchDirectRoot, { recursive: true, force: true });
    rmSync(linesRoot, { recursive: true, force: true });
  });

  describe("workspace mode", () => {
    test("reads a file as raw bytes", async () => {
      const bytes = await run(
        Effect.flatMap(FileSystem.FileSystem, (fs) => fs.readFile("notes.txt")),
        "docs",
      );

      expect(new TextDecoder().decode(bytes)).toBe("hello\nworld\n");
    });

    test("reads a file at a nested path as a string", async () => {
      const text = await run(
        Effect.flatMap(FileSystem.FileSystem, (fs) => fs.readFileString("sub/deep.txt")),
        "docs",
      );

      expect(text).toBe("deep");
    });

    test("streams the bytes of a file", async () => {
      const text = await run(
        Effect.flatMap(FileSystem.FileSystem, (fs) =>
          Stream.mkString(Stream.decodeText(fs.stream("notes.txt"))),
        ),
        "docs",
      );

      expect(text).toBe("hello\nworld\n");
    });

    test("splits a file into lines without line endings", async () => {
      const lines = await run(
        Effect.flatMap(FileSystem.FileSystem, (fs) => Stream.runCollect(fs.readLines("notes.txt"))),
        "docs",
      );

      expect(Array.from(lines)).toEqual(["hello", "world"]);
    });

    test("keeps a trailing line without a line ending", async () => {
      const lines = await run(
        Effect.flatMap(FileSystem.FileSystem, (fs) => Stream.runCollect(fs.readLines("readme.md"))),
        "docs",
      );

      expect(Array.from(lines)).toEqual(["line one", "line two"]);
    });

    test("pages through a file longer than one line read", async () => {
      const lines = Array.from(
        await run(
          Effect.flatMap(FileSystem.FileSystem, (fs) =>
            Stream.runCollect(fs.readLines("many.txt")),
          ),
          "lines",
        ),
      );

      expect(lines.length).toBe(1500);
      expect(lines[0]).toBe("line 0");
      expect(lines[999]).toBe("line 999");
      expect(lines[1000]).toBe("line 1000");
      expect(lines[1499]).toBe("line 1499");
    });

    test("describes a file's metadata", async () => {
      const info = await run(
        Effect.flatMap(FileSystem.FileSystem, (fs) => fs.stat("notes.txt")),
        "docs",
      );

      expect(info.type).toBe("File");
      expect(info.size).toBe(12n);
      expect(Option.isSome(info.mtime)).toBe(true);
    });

    test("describes a directory's metadata", async () => {
      const info = await run(
        Effect.flatMap(FileSystem.FileSystem, (fs) => fs.stat("sub")),
        "docs",
      );

      expect(info.type).toBe("Directory");
      expect(info.size).toBe(0n);
    });

    test("reports existence for present and missing resources", async () => {
      const value = await run(
        Effect.gen(function* () {
          const fs = yield* FileSystem.FileSystem;

          return {
            present: yield* fs.exists("notes.txt"),
            missing: yield* fs.exists("missing.txt"),
          };
        }),
        "docs",
      );

      expect(value).toEqual({ present: true, missing: false });
    });

    test("lists the direct children of a directory", async () => {
      const entries = await run(
        Effect.flatMap(FileSystem.FileSystem, (fs) => fs.readDirectory("")),
        "docs",
      );

      expect(sorted(entries)).toEqual(["escape", "notes.txt", "readme.md", "sub"]);
    });

    test("addressing a subdirectory lists its children with their path", async () => {
      const entries = await run(
        Effect.flatMap(FileSystem.FileSystem, (fs) => fs.readDirectory("sub")),
        "docs",
      );

      expect(sorted(entries)).toEqual(["sub/deep.txt"]);
    });

    test("lists a directory recursively", async () => {
      const entries = await run(
        Effect.flatMap(FileSystem.FileSystem, (fs) => fs.readDirectory("", { recursive: true })),
        "docs",
      );

      expect(sorted(entries)).toEqual(["escape", "notes.txt", "readme.md", "sub", "sub/deep.txt"]);
    });

    test("matches a recursive glob from the workspace root", async () => {
      const entries = await run(
        Effect.flatMap(FileSystem.FileSystem, (fs) => fs.glob("**/*.txt", { root: "" })),
        "docs",
      );

      expect(sorted(entries)).toEqual(["notes.txt", "sub/deep.txt"]);
    });

    test("restricts a glob to one path component", async () => {
      const entries = await run(
        Effect.flatMap(FileSystem.FileSystem, (fs) => fs.glob("*.md", { root: "" })),
        "docs",
      );

      expect(sorted(entries)).toEqual(["readme.md"]);
    });

    test("excludes a subtree from a glob", async () => {
      const entries = await run(
        Effect.flatMap(FileSystem.FileSystem, (fs) =>
          fs.glob("**/*", { root: "", exclude: ["sub"] }),
        ),
        "docs",
      );

      expect(sorted(entries)).toEqual(["escape", "notes.txt", "readme.md"]);
    });

    test("creates one directory", async () => {
      const made = await run(
        Effect.gen(function* () {
          const fs = yield* FileSystem.FileSystem;

          yield* fs.makeDirectory("created");

          return yield* fs.stat("created");
        }),
        "docs",
      );

      expect(made.type).toBe("Directory");
    });

    test("creates missing parents on request", async () => {
      const value = await run(
        Effect.gen(function* () {
          const fs = yield* FileSystem.FileSystem;

          yield* fs.makeDirectory("a/b/c", { recursive: true });

          return yield* fs.exists("a/b/c");
        }),
        "docs",
      );

      expect(value).toBe(true);
    });

    test("reports a missing file read as not found", async () => {
      const error = await failure((fs) => fs.readFile("missing.txt"), "docs");

      expect(error.reason._tag).toBe("NotFound");
    });

    test("reports an occupied directory as a conflict", async () => {
      const error = await failure(
        (fs) =>
          Effect.gen(function* () {
            yield* fs.makeDirectory("occupied");

            return yield* fs.makeDirectory("occupied");
          }),
        "docs",
      );

      expect(error.reason._tag).toBe("AlreadyExists");
    });

    test("reports a missing parent as not found", async () => {
      const error = await failure((fs) => fs.makeDirectory("ghost/child"), "docs");

      expect(error.reason._tag).toBe("NotFound");
    });

    test("rejects reading a directory as a file", async () => {
      const error = await failure((fs) => fs.readFile("sub"), "docs");

      expect(error.reason._tag).toBe("BadResource");
    });

    test("rejects listing a file as a directory", async () => {
      const error = await failure((fs) => fs.readDirectory("readme.md"), "docs");

      expect(error.reason._tag).toBe("BadResource");
    });

    test("confines a symlink that leaves the workspace", async () => {
      const error = await failure((fs) => fs.stat("escape"), "docs");

      expect(error.reason._tag).toBe("BadArgument");
      expect(error.reason.message).toContain("escapes workspace");
    });

    test("fails an operation the server does not implement yet", async () => {
      const error = await failure((fs) => fs.remove("notes.txt"), "docs");

      expect(error.reason._tag).toBe("Unknown");
      expect(error.reason.message).toContain("does not implement");
    });

    test("rejects a ranged stream the protocol cannot express", async () => {
      const error = await failure(
        (fs) => Stream.runCollect(fs.stream("notes.txt", { offset: 0 })),
        "docs",
      );

      expect(error.reason._tag).toBe("Unknown");
      expect(error.reason.message).toContain("offset");
    });
  });

  describe("direct mode", () => {
    test("reads a file by absolute path", async () => {
      const text = await run(
        Effect.flatMap(FileSystem.FileSystem, (fs) =>
          fs.readFileString(join(directRoot, "hello.txt")),
        ),
      );

      expect(text).toBe("hello");
    });

    test("describes a file by absolute path", async () => {
      const info = await run(
        Effect.flatMap(FileSystem.FileSystem, (fs) => fs.stat(join(directRoot, "nested"))),
      );

      expect(info.type).toBe("Directory");
    });

    test("lists absolute children of an absolute directory", async () => {
      const entries = await run(
        Effect.flatMap(FileSystem.FileSystem, (fs) => fs.readDirectory(directRoot)),
      );

      expect(sorted(entries)).toEqual([join(directRoot, "hello.txt"), join(directRoot, "nested")]);
    });

    test("matches a glob by absolute root and returns absolute paths", async () => {
      const entries = await run(
        Effect.flatMap(FileSystem.FileSystem, (fs) => fs.glob("**/*.txt", { root: directRoot })),
      );

      expect(sorted(entries)).toEqual([
        join(directRoot, "hello.txt"),
        join(directRoot, "nested", "deep.txt"),
      ]);
    });

    test("creates a directory by absolute path", async () => {
      const value = await run(
        Effect.gen(function* () {
          const fs = yield* FileSystem.FileSystem;

          yield* fs.makeDirectory(join(directRoot, "made"));

          return yield* fs.exists(join(directRoot, "made"));
        }),
      );

      expect(value).toBe(true);
    });

    test("rejects a relative path before sending the request", async () => {
      const error = await failure((fs) => fs.readFile("relative.txt"));

      expect(error.reason._tag).toBe("BadArgument");
      expect(error.reason.message).toContain("must be absolute");
    });
  });

  describe("watch", () => {
    test("reports a creation under a workspace directory", async () => {
      const event = await watchOnce(
        "",
        (current) => current.path === "made" && current._tag === "Create",
        Effect.sync(() => {
          mkdirSync(join(watchRoot, "made"));
        }),
        "watch",
      );

      expect(event).toEqual({ _tag: "Create", path: "made" });
    });

    test("reports an update to an existing file", async () => {
      const seed = join(watchRoot, "seed.txt");

      const event = await watchOnce(
        "",
        (current) => current.path === "seed.txt" && current._tag === "Update",
        Effect.gen(function* () {
          writeFileSync(seed, "one");
          yield* Effect.sleep(Duration.millis(200));
          appendFileSync(seed, "two");
        }),
        "watch",
      );

      expect(event).toEqual({ _tag: "Update", path: "seed.txt" });
    });

    test("reports a removal", async () => {
      const doomed = join(watchRoot, "doomed.txt");

      const event = await watchOnce(
        "",
        (current) => current.path === "doomed.txt" && current._tag === "Remove",
        Effect.gen(function* () {
          writeFileSync(doomed, "x");
          yield* Effect.sleep(Duration.millis(200));
          rmSync(doomed);
        }),
        "watch",
      );

      expect(event).toEqual({ _tag: "Remove", path: "doomed.txt" });
    });

    test("reports changes deeper in the tree when recursive", async () => {
      const event = await watchOnce(
        "",
        (current) => current.path === "tree/a/deep.txt" && current._tag !== "Remove",
        Effect.sync(() => {
          mkdirSync(join(watchRoot, "tree", "a"), { recursive: true });
          writeFileSync(join(watchRoot, "tree", "a", "deep.txt"), "deep");
        }),
        "watch",
        { recursive: true },
      );

      expect(event.path).toBe("tree/a/deep.txt");
      expect(["Create", "Update"]).toContain(event._tag);
    });

    test("keeps the workspace prefix when watching a subdirectory", async () => {
      mkdirSync(join(watchRoot, "elsewhere"));

      const event = await watchOnce(
        "elsewhere",
        (current) => current.path === "elsewhere/here.txt",
        Effect.sync(() => {
          writeFileSync(join(watchRoot, "elsewhere", "here.txt"), "hi");
        }),
        "watch",
      );

      expect(event.path).toBe("elsewhere/here.txt");
    });

    test("reports an absolute path in direct mode", async () => {
      const created = join(watchDirectRoot, "direct.txt");

      const event = await watchOnce(
        watchDirectRoot,
        (current) => current.path === created,
        Effect.sync(() => {
          writeFileSync(created, "hi");
        }),
      );

      expect(event.path).toBe(created);
    });

    test("reports a missing watch target as not found", async () => {
      const error = await failure((fs) => Stream.runCollect(fs.watch("ghost")), "watch");

      expect(error.reason._tag).toBe("NotFound");
    });

    test("rejects a relative watch path before sending the request", async () => {
      const error = await failure((fs) => Stream.runCollect(fs.watch("relative")));

      expect(error.reason._tag).toBe("BadArgument");
      expect(error.reason.message).toContain("must be absolute");
    });
  });
});
