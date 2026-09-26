import { execFileSync, spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, realpathSync, rmSync } from "node:fs";
import { createServer, connect } from "node:net";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

import { Effect, Layer, Option, Queue } from "effect";
import { QuitError } from "effect/Terminal";
import { afterAll, beforeAll, describe, expect, test } from "vite-plus/test";

import { ApiError, layerFetch } from "../src/internal/client.ts";
import * as Terminal from "../src/Terminal.ts";

/**
 * End-to-end wiring check between this package's `Terminal` client and the Rust
 * pty module: real HTTP, a real pty process, real WebSocket frames. The server
 * is built from the source tree and spawned on a private port with a throwaway
 * root and a registered workspace.
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

const terminalLayer = (workspace: string | undefined, options: Terminal.TerminalOptions) =>
  (workspace === undefined
    ? Terminal.layer(options)
    : Terminal.layerForWorkspace({ workspace, ...options })
  ).pipe(Layer.provide(layerFetch({ baseUrl })));

const runTerminal = <A, E>(
  program: Effect.Effect<A, E, Terminal.Terminal>,
  workspace?: string,
  options: Terminal.TerminalOptions = {},
): Promise<A> => Effect.runPromise(Effect.provide(program, terminalLayer(workspace, options)));

/** Reads until the session ends, treating the terminal close as the end of output. */
const drainLines = (terminal: Terminal.Terminal): Effect.Effect<ReadonlyArray<string>> =>
  Effect.gen(function* () {
    const lines: Array<string> = [];

    while (true) {
      const next = yield* Effect.option(terminal.readLine);

      if (Option.isNone(next)) return lines;

      lines.push(next.value);
    }
  });

/** Concatenates the text of every input event until the session ends. */
const drainInput = (terminal: Terminal.Terminal): Effect.Effect<string> =>
  Effect.gen(function* () {
    const inputs = yield* Effect.scoped(terminal.readInput);
    let text = "";

    while (true) {
      const next = yield* Effect.option(Queue.take(inputs));

      if (Option.isNone(next)) return text;

      text += Option.getOrUndefined(next.value.input) ?? "";
    }
  });

const readLine = Effect.flatMap(Terminal.Terminal, (terminal) => terminal.readLine);

describe.skipIf(!hasCargo && !existsSync(serverBinary))("Terminal ↔ pty", () => {
  let server: Server | undefined;
  let root = "";
  let workspaceRoot = "";
  let directRoot = "";

  beforeAll(async () => {
    if (hasCargo) {
      execFileSync("cargo", ["build", "--manifest-path", manifest], {
        cwd: repoRoot,
        stdio: "inherit",
      });
    }

    root = mkdtempSync(join(tmpdir(), "sbx-term-root-"));
    workspaceRoot = mkdtempSync(join(tmpdir(), "sbx-term-ws-"));
    directRoot = mkdtempSync(join(tmpdir(), "sbx-term-direct-"));
    mkdirSync(join(workspaceRoot, "sub"));

    server = await startServer(root);
    baseUrl = server.baseUrl;
    await registerWorkspace(server, "docs", workspaceRoot);
  }, 600_000);

  afterAll(async () => {
    await server?.stop();
    rmSync(root, { recursive: true, force: true });
    rmSync(workspaceRoot, { recursive: true, force: true });
    rmSync(directRoot, { recursive: true, force: true });
  });

  test("reports the configured size and applies it to the pty", async () => {
    const value = await runTerminal(
      Effect.gen(function* () {
        const terminal = yield* Terminal.Terminal;

        return {
          columns: yield* terminal.columns,
          rows: yield* terminal.rows,
          size: yield* terminal.readLine,
        };
      }),
      undefined,
      { command: "sh", args: ["-c", "stty size"], size: { rows: 30, columns: 100 } },
    );

    expect(value).toEqual({ columns: 100, rows: 30, size: "30 100" });
  });

  test("defaults the command to sh and the size to 24x80", async () => {
    const line = await runTerminal(readLine, undefined, { args: ["-c", "stty size"] });

    expect(line).toBe("24 80");
  });

  test("resolves a bare command through PATH", async () => {
    const line = await runTerminal(readLine, undefined, { command: "echo", args: ["path-ok"] });

    expect(line).toBe("path-ok");
  });

  test("runs in an absolute cwd in direct mode", async () => {
    const line = await runTerminal(readLine, undefined, {
      command: "sh",
      args: ["-c", "pwd"],
      cwd: directRoot,
    });

    expect(line).toBe(realpathSync(directRoot));
  });

  test("resolves a workspace-relative cwd", async () => {
    const line = await runTerminal(readLine, "docs", {
      command: "sh",
      args: ["-c", "pwd"],
      cwd: "sub",
    });

    expect(line).toBe(realpathSync(join(workspaceRoot, "sub")));
  });

  test("injects the workspace variables", async () => {
    const line = await runTerminal(readLine, "docs", {
      command: "sh",
      args: ["-c", 'printf %s "$WORKSPACE_DOCS"'],
    });

    expect(line).toBe(realpathSync(workspaceRoot));
  });

  test("layers the request env over the inherited one", async () => {
    const line = await runTerminal(readLine, undefined, {
      command: "sh",
      args: ["-c", 'printf %s "$SBXTKT_TERM_MARK"'],
      env: { SBXTKT_TERM_MARK: "marked" },
    });

    expect(line).toBe("marked");
  });

  test("writes display text to the process stdin", async () => {
    const lines = await runTerminal(
      Effect.gen(function* () {
        const terminal = yield* Terminal.Terminal;

        yield* terminal.display("value\n");

        return yield* drainLines(terminal);
      }),
      undefined,
      { command: "sh", args: ["-c", 'read line; printf "got:%s\\n" "$line"'] },
    );

    // The pty echoes the input before the command's own output, so the result is
    // the last line rather than the only one.
    expect(lines.at(-1)).toBe("got:value");
  });

  test("folds stderr into stdout", async () => {
    const lines = await runTerminal(Effect.flatMap(Terminal.Terminal, drainLines), undefined, {
      command: "sh",
      args: ["-c", "printf err 1>&2"],
    });

    expect(lines).toEqual(["err"]);
  });

  test("splits stdout into lines", async () => {
    const lines = await runTerminal(Effect.flatMap(Terminal.Terminal, drainLines), undefined, {
      command: "sh",
      args: ["-c", "printf 'a\\nb\\n'"],
    });

    expect(lines).toEqual(["a", "b"]);
  });

  test("surfaces stdout chunks as input events", async () => {
    const text = await runTerminal(Effect.flatMap(Terminal.Terminal, drainInput), undefined, {
      command: "sh",
      args: ["-c", "printf abc"],
    });

    expect(text).toBe("abc");
  });

  test("ends readLine with a quit error when the process exits", async () => {
    const value = await runTerminal(
      Effect.gen(function* () {
        const terminal = yield* Terminal.Terminal;
        const first = yield* terminal.readLine;
        const quit = yield* Effect.flip(terminal.readLine);

        return { first, quit };
      }),
      undefined,
      { command: "sh", args: ["-c", "printf x; exit 3"] },
    );

    expect(value.first).toBe("x");
    expect(value.quit).toBeInstanceOf(QuitError);
  });

  test("reports an unknown workspace as an API error", async () => {
    await expect(
      runTerminal(
        Effect.flatMap(Terminal.Terminal, (terminal) => terminal.columns),
        "missing",
      ),
    ).rejects.toBeInstanceOf(ApiError);
  });

  test("maps an empty command to a bad request", async () => {
    await expect(
      runTerminal(
        Effect.flatMap(Terminal.Terminal, (terminal) => terminal.columns),
        undefined,
        {
          command: "   ",
        },
      ),
    ).rejects.toBeInstanceOf(ApiError);
  });
});
