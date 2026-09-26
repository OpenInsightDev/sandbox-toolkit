import { execFileSync, spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, realpathSync, rmSync } from "node:fs";
import { createServer, connect } from "node:net";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

import { Effect, Layer, Stream } from "effect";
import { afterAll, beforeAll, describe, expect, test } from "vite-plus/test";

import { ApiError, layerFetch } from "../src/internal/client.ts";
import * as Process from "../src/Process.ts";

/**
 * End-to-end wiring check between this package's `Process` client and the Rust
 * process module: real HTTP, real subprocesses, real frame stream, plus the raw
 * exec and pty routes the typed client never reaches. The server is built from
 * the source tree and spawned on a private port with a throwaway root.
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

interface Collected {
  readonly stdout: string;
  readonly stderr: string;
  readonly exitCode: number | undefined;
}

const collectEvents = (events: ReadonlyArray<Process.ProcessEvent>): Collected => {
  const decoder = new TextDecoder();
  let stdout = "";
  let stderr = "";
  let exitCode: number | undefined;

  for (const event of events) {
    switch (event._tag) {
      case "Stdout":
        stdout += decoder.decode(event.data);
        break;
      case "Stderr":
        stderr += decoder.decode(event.data);
        break;
      case "Exit":
        exitCode = event.exitCode;
        break;
    }
  }

  return { stdout, stderr, exitCode };
};

let baseUrl = "";

const processLayer = (workspace?: string) =>
  (workspace === undefined ? Process.layer : Process.layerForWorkspace({ workspace })).pipe(
    Layer.provide(layerFetch({ baseUrl })),
  );

const run = <A, E>(program: Effect.Effect<A, E, Process.Process>, workspace?: string): Promise<A> =>
  Effect.runPromise(Effect.provide(program, processLayer(workspace)));

const execStreamContentType = "application/vnd.sandbox-toolkit.exec-stream";

/** A raw request, for the wire details the typed client never produces. */
const postJson = (path: string, body: unknown): Promise<Response> =>
  fetch(`${baseUrl}${path}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });

const errorCode = async (response: Response): Promise<string> =>
  ((await response.json()) as { error: { code: string } }).error.code;

describe.skipIf(!hasCargo && !existsSync(serverBinary))("Process ↔ exec and pty", () => {
  let server: Server | undefined;
  let root = "";
  let workspaceRoot = "";

  beforeAll(async () => {
    if (hasCargo) {
      execFileSync("cargo", ["build", "--manifest-path", manifest], {
        cwd: repoRoot,
        stdio: "inherit",
      });
    }

    root = mkdtempSync(join(tmpdir(), "sbx-root-"));
    workspaceRoot = mkdtempSync(join(tmpdir(), "sbx-ws-"));
    mkdirSync(join(workspaceRoot, "sub"));

    server = await startServer(root);
    baseUrl = server.baseUrl;
    await registerWorkspace(server, "docs", workspaceRoot);
  }, 600_000);

  afterAll(async () => {
    await server?.stop();
    rmSync(root, { recursive: true, force: true });
    rmSync(workspaceRoot, { recursive: true, force: true });
  });

  test("collects a direct result for a short exec", async () => {
    const value = await run(
      Effect.gen(function* () {
        const process = yield* Process.Process;

        return yield* process.string({ command: "echo", args: ["hi"] });
      }),
    );

    expect(value).toBe("hi\n");
  });

  test("reports a non-zero exit code", async () => {
    const value = await run(
      Effect.gen(function* () {
        const process = yield* Process.Process;

        return yield* process.exitCode({ command: "sh", args: ["-c", "exit 7"] });
      }),
    );

    expect(value).toBe(7);
  });

  test("keeps stderr apart and folds it in on request", async () => {
    const [separate, folded] = await run(
      Effect.gen(function* () {
        const process = yield* Process.Process;
        const command = { command: "sh", args: ["-c", "printf out; printf err 1>&2"] };

        return [
          yield* process.string(command),
          yield* process.string(command, { includeStderr: true }),
        ] as const;
      }),
    );

    expect(separate).toBe("out");
    expect(folded).toBe("outerr");
  });

  test("layers a request env over the inherited one", async () => {
    const value = await run(
      Effect.gen(function* () {
        const process = yield* Process.Process;

        return yield* process.string({
          command: "sh",
          args: ["-c", 'printf %s "$SBXTKT_E2E_MARK"'],
          options: { env: { SBXTKT_E2E_MARK: "marked" } },
        });
      }),
    );

    expect(value).toBe("marked");
  });

  test("upgrades a command that outlives the probe to the frame stream", async () => {
    const value = await run(
      Effect.gen(function* () {
        const process = yield* Process.Process;

        const events = yield* Stream.runCollect(
          process.stream({ command: "sh", args: ["-c", "printf out; printf err 1>&2; sleep 0.4"] }),
        );

        return collectEvents(Array.from(events));
      }),
    );

    expect(value).toEqual({ stdout: "out", stderr: "err", exitCode: 0 });
  });

  test("streams a non-zero exit status", async () => {
    const value = await run(
      Effect.gen(function* () {
        const process = yield* Process.Process;

        const events = yield* Stream.runCollect(
          process.stream({ command: "sh", args: ["-c", "sleep 0.3; exit 5"] }),
        );

        return collectEvents(Array.from(events));
      }),
    );

    expect(value.exitCode).toBe(5);
  });

  test("carries raw bytes through the frame stream", async () => {
    const bytes = await run(
      Effect.gen(function* () {
        const process = yield* Process.Process;

        const events = yield* Stream.runCollect(
          process.stream({ command: "sh", args: ["-c", "printf '\\377'; sleep 0.3"] }),
        );

        const chunks: Array<Uint8Array> = [];

        for (const event of events) {
          if (event._tag === "Stdout") chunks.push(event.data);
        }

        return chunks.flatMap((chunk) => Array.from(chunk));
      }),
    );

    expect(bytes).toEqual([0xff]);
  });

  test("splits output into lines", async () => {
    const value = await run(
      Effect.gen(function* () {
        const process = yield* Process.Process;

        return yield* Stream.runCollect(
          process.lines({ command: "sh", args: ["-c", "printf 'a\\nbb\\n'"] }),
        );
      }),
    );

    expect(Array.from(value)).toEqual(["a", "bb"]);
  });

  test("runs a shell template with interpolated values", async () => {
    const value = await run(
      Effect.gen(function* () {
        const process = yield* Process.Process;

        return yield* process.$`printf %s ${"a"}${"b"}`;
      }),
    );

    expect(value).toBe("ab");
  });

  test("exec returns a direct result for a fast command", async () => {
    const value = await run(
      Effect.gen(function* () {
        const process = yield* Process.Process;
        const outcome = yield* process.exec({ command: "echo", args: ["hi"] });

        return Stream.isStream(outcome)
          ? "stream"
          : { stdout: yield* Stream.mkString(Stream.decodeText(outcome.stdout)) };
      }),
    );

    expect(value).toEqual({ stdout: "hi\n" });
  });

  test("exec returns the frame stream when the command outlives the probe", async () => {
    const value = await run(
      Effect.gen(function* () {
        const process = yield* Process.Process;

        const outcome = yield* process.exec({
          command: "sh",
          args: ["-c", "printf slow; sleep 0.8"],
        });

        return Stream.isStream(outcome)
          ? collectEvents(Array.from(yield* Stream.runCollect(outcome)))
          : "result";
      }),
    );

    expect(value).toEqual({ stdout: "slow", stderr: "", exitCode: 0 });
  });

  test("maps a spawn failure to the shared API error envelope", async () => {
    const error = await run(
      Effect.gen(function* () {
        const process = yield* Process.Process;

        return yield* Effect.flip(
          process.exitCode({ command: "sbx-e2e-missing-binary", args: [] }),
        );
      }),
    );

    expect(error).toBeInstanceOf(ApiError);
    expect((error as ApiError).code).toBe("bad_request");
  });

  test("maps an empty command to a bad request", async () => {
    const error = await run(
      Effect.gen(function* () {
        const process = yield* Process.Process;

        return yield* Effect.flip(process.string({ command: "   ", args: [] }));
      }),
    );

    expect(error).toBeInstanceOf(ApiError);
  });

  test("resolves a workspace-relative cwd and injects the workspace variable", async () => {
    const value = await run(
      Effect.gen(function* () {
        const process = yield* Process.Process;

        return {
          sub: yield* process.string({ command: "pwd", args: [], options: { cwd: "sub" } }),
          variable: yield* process.string({
            command: "sh",
            args: ["-c", 'printf %s "$WORKSPACE_DOCS"'],
          }),
        };
      }),
      "docs",
    );

    expect(value.sub.trim()).toBe(realpathSync(join(workspaceRoot, "sub")));
    expect(value.variable).toBe(realpathSync(workspaceRoot));
  });

  test("rejects a cwd that escapes the workspace", async () => {
    const error = await run(
      Effect.gen(function* () {
        const process = yield* Process.Process;

        return yield* Effect.flip(
          process.string({ command: "pwd", args: [], options: { cwd: "../" } }),
        );
      }),
      "docs",
    );

    expect(error).toBeInstanceOf(ApiError);
  });

  test("collects a command that outlives the probe into a result", async () => {
    const value = await run(
      Effect.gen(function* () {
        const process = yield* Process.Process;

        const spawned = yield* process.result({
          command: "sh",
          args: ["-c", "printf out; printf err 1>&2; sleep 0.4"],
        });

        return {
          stdout: yield* Stream.mkString(Stream.decodeText(spawned.stdout)),
          stderr: yield* Stream.mkString(Stream.decodeText(spawned.stderr)),
          exitCode: yield* spawned.exitCode,
        };
      }),
    );

    expect(value).toEqual({ stdout: "out", stderr: "err", exitCode: 0 });
  });

  test("reports the exit code of a command that outlives the probe", async () => {
    const value = await run(
      Effect.gen(function* () {
        const process = yield* Process.Process;

        return yield* process.exitCode({ command: "sh", args: ["-c", "sleep 0.3; exit 9"] });
      }),
    );

    expect(value).toBe(9);
  });

  test("runs a shell template that outlives the probe", async () => {
    const value = await run(
      Effect.gen(function* () {
        const process = yield* Process.Process;

        return yield* process.$({ wait: 0 })`printf shell; sleep 0.2`;
      }),
    );

    expect(value).toBe("shell");
  });

  test("runs a shell template whose output exceeds the direct response limit", async () => {
    const value = await run(
      Effect.gen(function* () {
        const process = yield* Process.Process;

        return yield* process.$`yes x | head -c 4500000`;
      }),
    );

    expect(value.length).toBe(4_500_000);
  });

  test("returns only the stdout of a shell template", async () => {
    const value = await run(
      Effect.gen(function* () {
        const process = yield* Process.Process;

        return yield* process.$`printf out; printf err 1>&2`;
      }),
    );

    expect(value).toBe("out");
  });

  test("fails a shell template with a non-zero exit and its output", async () => {
    const error = await run(
      Effect.gen(function* () {
        const process = yield* Process.Process;

        return yield* Effect.flip(process.$`printf out; printf err 1>&2; exit 3`);
      }),
    );

    expect(error).toBeInstanceOf(Process.CommandExitError);
    expect(error).toMatchObject({ exitCode: 3, stdout: "out", stderr: "err" });
  });

  test("carries the streamed output of a failing shell template", async () => {
    const error = await run(
      Effect.gen(function* () {
        const process = yield* Process.Process;

        return yield* Effect.flip(
          process.$({ wait: 0 })`printf out; printf err 1>&2; sleep 0.2; exit 3`,
        );
      }),
    );

    expect(error).toBeInstanceOf(Process.CommandExitError);
    expect(error).toMatchObject({ exitCode: 3, stdout: "out", stderr: "err" });
  });

  test("applies the shell template's cwd and env", async () => {
    const value = await run(
      Effect.gen(function* () {
        const process = yield* Process.Process;

        return {
          cwd: yield* process.$({ cwd: "sub" })`pwd`,
          env: yield* process.$({ env: { SBXTKT_E2E_SHELL: "ok" } })`printf %s "$SBXTKT_E2E_SHELL"`,
        };
      }),
      "docs",
    );

    expect(value.cwd.trim()).toBe(realpathSync(join(workspaceRoot, "sub")));
    expect(value.env).toBe("ok");
  });

  test("splits CRLF output into lines", async () => {
    const value = await run(
      Effect.gen(function* () {
        const process = yield* Process.Process;

        return yield* Stream.runCollect(
          process.lines({ command: "sh", args: ["-c", "printf 'a\\r\\nb\\nlast'"] }),
        );
      }),
    );

    expect(Array.from(value)).toEqual(["a", "b", "last"]);
  });

  test("collects output larger than the direct response limit", async () => {
    const value = await run(
      Effect.gen(function* () {
        const process = yield* Process.Process;

        return yield* process.string({ command: "sh", args: ["-c", "yes x | head -c 4500000"] });
      }),
    );

    expect(value.length).toBe(4_500_000);
  });

  describe("raw HTTP contract", () => {
    test("answers a fast command with JSON and the flattened status", async () => {
      const response = await postJson("/exec", {
        command: "sh",
        args: ["-c", "printf out; printf err 1>&2; exit 3"],
        wait: 5000,
      });

      expect(response.status).toBe(200);
      expect(response.headers.get("content-type")).toContain("application/json");
      expect(await response.json()).toEqual({
        status: "exited",
        exit_code: 3,
        stdout: "out",
        stderr: "err",
      });
    });

    test("accepts the shell payload on the same exec route", async () => {
      const response = await postJson("/exec", {
        format: "shell",
        script: "printf hi",
        wait: 5000,
      });

      expect(await response.json()).toMatchObject({
        status: "exited",
        exit_code: 0,
        stdout: "hi",
      });
    });

    test("streams immediately when wait is absent", async () => {
      const response = await postJson("/exec", {
        command: "sh",
        args: ["-c", "printf out"],
      });

      expect(response.headers.get("content-type")).toContain(execStreamContentType);

      await response.arrayBuffer();
    });

    test("upgrades once the output exceeds the direct response limit", async () => {
      const response = await postJson("/exec", {
        command: "sh",
        args: ["-c", "yes x | head -c 4500000"],
        wait: 5000,
      });

      expect(response.headers.get("content-type")).toContain(execStreamContentType);

      await response.arrayBuffer();
    });

    test("rejects an unknown format", async () => {
      const response = await postJson("/exec", { format: "python", script: "print(1)" });

      expect(response.status).toBe(422);
      expect(await errorCode(response)).toBe("unsupported_type");
    });

    test("rejects a body that fails the selected schema", async () => {
      const response = await postJson("/exec", {});

      expect(response.status).toBe(422);
      expect(await errorCode(response)).toBe("invalid_request");
    });

    test("reports an unknown workspace", async () => {
      const response = await postJson("/workspaces/missing/exec", { command: "true" });

      expect(response.status).toBe(404);
      expect(await errorCode(response)).toBe("not_found");
    });

    test("injects the workspace variable in direct mode", async () => {
      const response = await postJson("/exec", {
        command: "sh",
        args: ["-c", 'printf %s "$WORKSPACE_DOCS"'],
        wait: 5000,
      });

      const body = (await response.json()) as { stdout: string };

      expect(body.stdout).toBe(realpathSync(workspaceRoot));
    });

    test("wires the pty create route to the not-implemented result", async () => {
      const response = await postJson("/pty", { command: "sh" });

      expect(response.status).toBe(501);
      expect(await errorCode(response)).toBe("not_implemented");
    });

    test("rejects a pty create on an unknown workspace before the handler", async () => {
      const response = await postJson("/workspaces/missing/pty", { command: "sh" });

      expect(response.status).toBe(404);
      expect(await errorCode(response)).toBe("not_found");
    });

    test("rejects a pty attach that is not a WebSocket handshake", async () => {
      const response = await fetch(`${baseUrl}/pty/session-1`);

      expect(response.status).toBe(400);
    });
  });
});
