import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { Effect, type Layer } from "effect";
import { describe, expect, it } from "vite-plus/test";

import { SandboxToolkit } from "../src/client.ts";
import type { SandboxToolkitError } from "../src/errors.ts";

/**
 * Skipped unless `SANDBOX_TOOLKIT_BASE_URL` points at a server:
 *
 * ```sh
 * SANDBOX_TOOLKIT_BASE_URL=http://127.0.0.1:8000 vp test
 * ```
 */
const baseUrl = process.env["SANDBOX_TOOLKIT_BASE_URL"];

// `describe.skipIf` does not narrow `baseUrl`.
const liveLayer = (): Layer.Layer<SandboxToolkit> => {
  if (baseUrl === undefined) {
    throw new Error("SANDBOX_TOOLKIT_BASE_URL must be set to run the live suite");
  }
  return SandboxToolkit.layer({ baseUrl });
};

describe.skipIf(baseUrl === undefined)("SandboxToolkit (live server)", () => {
  const run = <A, E>(effect: Effect.Effect<A, E, SandboxToolkit>): Promise<A> =>
    Effect.runPromise(effect.pipe(Effect.provide(liveLayer())));

  const runError = <A>(
    effect: Effect.Effect<A, SandboxToolkitError, SandboxToolkit>,
  ): Promise<SandboxToolkitError> =>
    Effect.runPromise(effect.pipe(Effect.provide(liveLayer()), Effect.flip));

  const withClient = <A, E>(
    f: (client: SandboxToolkit["Service"]) => Effect.Effect<A, E>,
  ): Effect.Effect<A, E, SandboxToolkit> =>
    Effect.gen(function* () {
      const client = yield* SandboxToolkit;
      return yield* f(client);
    });

  it("reports health", async () => {
    const health = await run(withClient((client) => client.health));

    expect(health.status).toBe("ok");
    expect(typeof health.uptimeSeconds).toBe("number");
  });

  it("lists the bundled tools", async () => {
    const tools = await run(withClient((client) => client.listTools));

    expect(tools.map((tool) => tool.name)).toEqual(expect.arrayContaining(["fd", "rg"]));
  });

  it("describes a bundled tool and rejects an unknown one", async () => {
    const described = await run(withClient((client) => client.describeTool({ name: "rg" })));
    expect(described.name).toBe("rg");
    expect(described.path).toContain("rg");

    const error = await runError(
      withClient((client) => client.describeTool({ name: "definitely-not-a-tool" })),
    );
    expect(error.reason._tag).toBe("NotFoundError");
  });

  it("reads a window of lines from a real file", async () => {
    const directory = await mkdtemp(join(tmpdir(), "sandbox-toolkit-sdk-"));
    const path = join(directory, "sample.txt");
    await writeFile(path, "alpha\nbravo\ncharlie\ndelta\n");

    try {
      const result = await run(
        withClient((client) => client.readFile({ path, offset: 1, limit: 2 })),
      );

      expect(result.path).toBe(path);
      expect(result.lines).toEqual([
        { number: 2, text: "bravo" },
        { number: 3, text: "charlie" },
      ]);
      expect(result.truncated).toBe(true);
      expect(result.nextOffset).toBe(3);

      const nextOffset = result.nextOffset;
      if (nextOffset === undefined) throw new Error("expected a follow-up offset");

      const last = await run(
        withClient((client) => client.readFile({ path, offset: nextOffset, limit: 2 })),
      );
      expect(last.lines).toEqual([{ number: 4, text: "delta" }]);
      expect(last.truncated).toBe(false);
      expect(last.nextOffset).toBeUndefined();
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });

  it("rejects a relative path with a typed error", async () => {
    const error = await runError(withClient((client) => client.readFile({ path: "Cargo.toml" })));

    expect(error.reason._tag).toBe("InvalidRequestError");
    expect(error.reason.message).toBe("path must be absolute: Cargo.toml");
  });

  it("reports a missing file as not found", async () => {
    const error = await runError(
      withClient((client) =>
        client.readFile({ path: "/tmp/definitely-not-here-sandbox-toolkit.txt" }),
      ),
    );

    expect(error.reason._tag).toBe("NotFoundError");
  });
});
