import { execFileSync } from "node:child_process";
import { createServer } from "node:net";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { Layer } from "effect";
import { NodeHttpClient } from "@effect/platform-node";
import { layer } from "../src/Workspace.ts";

/** How long the sandbox gets to answer before a start is called a failure. */
const READY_TIMEOUT_MS = 30_000;
const POLL_INTERVAL_MS = 100;

/** How long one container CLI call may take before it is called a failure. */
const CALL_TIMEOUT_MS = 30_000;

const repo = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..");
const script = join(repo, "tools", "test", "sandbox.sh");

/** Distinct container names within one run, so parallel files never collide. */
let serial = 0;
const unique = (tag: string): string => `${tag}-${process.pid}-${serial++}`;

/**
 * Runs `tools/test/sandbox.sh`, which owns the host's container runtime.
 *
 * The output is captured rather than inherited: a container CLI that leaves a
 * logger behind would otherwise hold the test runner's stdout open, and the run
 * would look hung after the tests had already finished.
 *
 * Every call is bounded. These are `start`, `stop` and `logs`, which answer in
 * seconds, so a runtime that stalls is a failure the runner can report instead
 * of a synchronous call that blocks its event loop past every timeout.
 */
const sandbox = (args: ReadonlyArray<string>): string =>
  execFileSync("sh", [script, ...args], {
    cwd: repo,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
    timeout: CALL_TIMEOUT_MS,
  }).trim();

/** A port the sandbox can publish; the caller binds it first to learn it free. */
const freePort = (): Promise<number> =>
  new Promise((resolve, reject) => {
    const probe = createServer();
    probe.on("error", reject);
    probe.listen(0, "127.0.0.1", () => {
      const address = probe.address();
      if (address === null || typeof address === "string") {
        probe.close();
        reject(new Error("the probe socket has no port"));
        return;
      }

      probe.close(() => resolve(address.port));
    });
  });

const sleep = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));

/**
 * Polls until the server answers, the way the Rust suite does: readiness is the
 * control-plane mount responding, not the container reporting started.
 */
const waitReady = async (baseUrl: string, name: string): Promise<void> => {
  const deadline = Date.now() + READY_TIMEOUT_MS;

  while (Date.now() < deadline) {
    try {
      const response = await fetch(`${baseUrl}/workspaces`, {
        signal: AbortSignal.timeout(2_000),
      });
      if (response.ok) {
        return;
      }
    } catch {
      // Not listening yet.
    }

    await sleep(POLL_INTERVAL_MS);
  }

  const log = sandbox(["logs", name]);
  sandbox(["stop", name]);
  throw new Error(
    `the sandbox did not answer ${baseUrl} within ${READY_TIMEOUT_MS} ms.\n` +
      "The server binary comes from the build task; re-run it past its cache " +
      `with \`vp run -w --no-cache rust:build:linux\` if it outlived the sandbox.\n${log}`,
  );
};

/** A running `sbxtkt serve`, reachable from the host at `baseUrl`. */
export interface Sandbox {
  readonly baseUrl: string;
  readonly stop: () => void;
}

/** Starts a sandbox, and stops it again if it never answers. */
export const startSandbox = async (): Promise<Sandbox> => {
  const name = unique("sbxtkt-sandbox");
  const port = await freePort();

  sandbox(["start", String(port), name]);

  const baseUrl = `http://127.0.0.1:${port}`;
  await waitReady(baseUrl, name);

  return { baseUrl, stop: () => sandbox(["stop", name]) };
};

/** The SDK wired to one sandbox, so a test provides a single layer. */
export const workspaceLayer = (baseUrl: string) =>
  layer({ baseUrl }).pipe(Layer.provide(NodeHttpClient.layerNodeHttp));
