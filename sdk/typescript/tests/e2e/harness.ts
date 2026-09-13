import { execFileSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { Effect, Stream } from "effect";
import { ClientLayer, makeClient, type Client } from "#/client.ts";
import type { OperationRequirements } from "#/operations.ts";

/**
 * End-to-end harness backed by the `rshs` WebDAV server container
 * (https://github.com/mogeko/rshs).
 *
 * Two Docker quirks shape this harness:
 *
 * 1. rshs batches `statx` through `io_uring`, which Docker's default seccomp
 *    profile blocks. The container therefore runs with
 *    `--security-opt seccomp=unconfined`, otherwise every directory listing is
 *    empty ("Cannot read directory").
 * 2. On Docker Desktop for macOS, recursively deleting and recreating entries
 *    of a host bind mount leaves the container with a stale view (later
 *    requests fail with `ENOENT`). The served tree therefore lives on a named
 *    Docker volume, and a small `busybox` sidecar resets it from fixtures that
 *    were copied into the VM once.
 *
 * Tests observe mutations through the WebDAV API rather than the host
 * filesystem.
 */
export const IMAGE = "ghcr.io/mogeko/rshs:latest";
export const RESET_IMAGE = "busybox:latest";
export const USERNAME = "admin";
export const PASSWORD = "pass";

const dirname = path.dirname(fileURLToPath(import.meta.url));
export const SERVER_CONTENTS = path.resolve(dirname, "fixtures/serverContents");

const suffix = `${process.pid}`;
const containerName = `webdav-client-e2e-${suffix}`;
const resetName = `webdav-client-e2e-reset-${suffix}`;
const volumeName = `webdav-client-e2e-data-${suffix}`;
let base = "";

const docker = (...args: Array<string>): string =>
    execFileSync("docker", args, { encoding: "utf8" }).trim();

const dockerQuiet = (...args: Array<string>): void => {
    try {
        execFileSync("docker", args, { stdio: "ignore" });
    } catch {
        // Best-effort cleanup.
    }
};

const imageAvailable = (image: string): boolean => {
    try {
        docker("image", "inspect", image);
        return true;
    } catch {
        return false;
    }
};

const ensureImage = (image: string): void => {
    if (!imageAvailable(image)) {
        docker("pull", image);
    }
};

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

const waitUntilReady = async (url: string): Promise<void> => {
    const deadline = Date.now() + 30_000;
    let lastError: unknown;
    while (Date.now() < deadline) {
        try {
            // A 401 is a valid readiness signal: the server is up and authenticating.
            await fetch(url, { method: "OPTIONS" });
            return;
        } catch (error) {
            lastError = error;
            await sleep(200);
        }
    }
    throw new Error(`rshs did not become ready at ${url}: ${String(lastError)}`);
};

/** Replace the served tree with a pristine copy of the fixtures. */
export const resetFixtures = (): void => {
    execFileSync(
        "docker",
        [
            "exec",
            resetName,
            "sh",
            "-c",
            "rm -rf /mnt/data/* /mnt/data/.[!.]* 2>/dev/null; cp -a /fixtures/. /mnt/data/",
        ],
        { stdio: "ignore" },
    );
};

export const baseUrl = (): string => base;

export const startServer = async (): Promise<void> => {
    ensureImage(IMAGE);
    ensureImage(RESET_IMAGE);

    dockerQuiet("rm", "-f", containerName);
    dockerQuiet("rm", "-f", resetName);
    dockerQuiet("volume", "rm", "-f", volumeName);
    docker("volume", "create", volumeName);

    // Long-lived sidecar that owns the served volume and a pristine fixture copy.
    docker(
        "run",
        "-d",
        "--name",
        resetName,
        "-v",
        `${volumeName}:/mnt/data`,
        RESET_IMAGE,
        "sleep",
        "infinity",
    );
    docker("exec", resetName, "mkdir", "-p", "/fixtures");
    docker("cp", `${SERVER_CONTENTS}/.`, `${resetName}:/fixtures/`);
    resetFixtures();

    docker(
        "run",
        "-d",
        "--rm",
        "--name",
        containerName,
        "--security-opt",
        "seccomp=unconfined",
        "-e",
        "RSHS_ROOT_DIR=/mnt/data",
        "-e",
        `RSHS_USERS=${USERNAME}:${PASSWORD}`,
        "-e",
        process.env.WEBDAV_E2E_KEEP === "1" ? "RSHS_LOG=debug" : "RSHS_LOG=warn",
        "-p",
        "127.0.0.1::8080",
        "-v",
        `${volumeName}:/mnt/data`,
        IMAGE,
    );

    const port = docker("port", containerName, "8080").split(":").pop();
    if (!port) {
        throw new Error("Unable to determine the published rshs port");
    }
    base = `http://127.0.0.1:${port}`;
    await waitUntilReady(base);
};

export const stopServer = (): void => {
    if (process.env.WEBDAV_E2E_KEEP === "1") {
        return;
    }
    dockerQuiet("rm", "-f", containerName);
    dockerQuiet("rm", "-f", resetName);
    dockerQuiet("volume", "rm", "-f", volumeName);
};

export const clientConfig = (overrides: Record<string, unknown> = {}) => ({
    remoteUrl: baseUrl(),
    username: USERNAME,
    password: PASSWORD,
    ...overrides,
});

export type ClientInstance = Client;

/** Run an Effect against a freshly constructed client bound to the container. */
export const withClient = <A, E>(
    fn: (client: ClientInstance) => Effect.Effect<A, E, OperationRequirements>,
    overrides: Record<string, unknown> = {},
): Effect.Effect<A, E, never> =>
    Effect.gen(function* () {
        const client = yield* makeClient();
        return yield* fn(client);
    }).pipe(Effect.provide(ClientLayer(clientConfig(overrides)))) as unknown as Effect.Effect<
        A,
        E,
        never
    >;

/** Capture the outgoing requests made through the global fetch used by the transport. */
export const captureRequests = () => {
    const original = globalThis.fetch;
    const requests: Array<{ url: string; method: string; headers: Record<string, string> }> = [];
    globalThis.fetch = async (input, init) => {
        const request = new Request(input, init);
        requests.push({
            url: request.url,
            method: request.method,
            headers: Object.fromEntries(request.headers.entries()),
        });
        return original(input, init);
    };
    return {
        requests,
        restore: () => {
            globalThis.fetch = original;
        },
    };
};

/** Drain an Effect stream of `Uint8Array` chunks into a single byte array. */
export const collectBytes = <E>(stream: Stream.Stream<Uint8Array, E>) =>
    Stream.runCollect(stream).pipe(
        Effect.map((chunks) =>
            Uint8Array.from(Array.from(chunks).flatMap((chunk) => Array.from(chunk))),
        ),
    );