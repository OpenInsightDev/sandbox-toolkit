import { Effect } from "effect";
import { describe, expect, it } from "@effect/vitest";
import { makeClient, ClientLayer } from "#/client.ts";
import { responseFromText, TestTransport } from "#/transport.ts";
import { AuthTest } from "#/auth.ts";
import { OperationsLive } from "#/operations.ts";
import { config } from "./operations/helpers.ts";
import { Layer } from "effect";

describe("WebDAV client facade", () => {
    it.effect("provides the auth service through the public client layer", () => {
        const originalFetch = globalThis.fetch;
        let request: Request | undefined;
        globalThis.fetch = async (input, init) => {
            request = new Request(input, init);
            return new Response("ok", { status: 200 });
        };

        return Effect.gen(function* () {
            const client = yield* makeClient();
            expect(yield* client.getFileContents("notes.txt", { format: "text" })).toBe("ok");
            expect(request?.method).toBe("GET");
            expect(request?.url).toBe("https://dav.example.test/root/notes.txt");
        }).pipe(
            Effect.provide(
                ClientLayer({
                    remoteUrl: "https://dav.example.test",
                    remoteBasePath: "/root",
                }),
            ),
            Effect.ensuring(
                Effect.sync(() => {
                    globalThis.fetch = originalFetch;
                }),
            ),
        );
    });

    it.effect("exposes every operation and delegates through the shared services", () => {
        const layer = Layer.mergeAll(
            OperationsLive({ config }),
            AuthTest(config),
            TestTransport({
                execute: (request) =>
                    Effect.succeed(
                        responseFromText({
                            url: request.url,
                            status: request.method === "HEAD" ? 200 : 204,
                        }),
                    ),
            }),
        );
        return Effect.gen(function* () {
            const client = yield* makeClient();
            expect(Object.keys(client).sort()).toEqual(
                [
                    "createDirectory",
                    "createReadStream",
                    "createWriteStream",
                    "customRequest",
                    "deleteFile",
                    "exists",
                    "getDAVCompliance",
                    "getDirectoryContents",
                    "getFileContents",
                    "getFileDownloadLink",
                    "getFileUploadLink",
                    "getQuota",
                    "lock",
                    "moveFile",
                    "partialUpdateFileContents",
                    "putFileContents",
                    "search",
                    "stat",
                    "unlock",
                    "copyFile",
                ].sort(),
            );
            expect(yield* client.exists("present.txt")).toBe(true);
            expect((yield* client.getFileDownloadLink("present.txt")).method).toBe("GET");
        }).pipe(Effect.provide(layer));
    });
});