import { Effect } from "effect";
import { describe, expect, it } from "@effect/vitest";
import { decodeWebDavConfigSync, makeClient } from "#/index.ts";

describe("package entrypoint", () => {
    it.effect("re-exports the public configuration and client API", () =>
        Effect.sync(() => {
            expect(decodeWebDavConfigSync({ remoteUrl: "https://dav.example.test" })).toMatchObject(
                {
                    remoteUrl: "https://dav.example.test",
                    authType: "none",
                },
            );
            expect(typeof makeClient).toBe("function");
        }),
    );
});