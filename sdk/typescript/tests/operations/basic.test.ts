import { Effect } from "effect";
import { describe, expect, it } from "@effect/vitest";
import {
    CopyFile,
    CreateDirectory,
    DeleteFile,
    Exists,
    GetDavCompliance,
    MoveFile,
    Unlock,
} from "#/operations.ts";
import { responseFromText } from "#/transport.ts";
import { makeLayer } from "./helpers.ts";

describe("WebDAV basic operations", () => {
    it.effect("exists returns false for a missing resource and delete accepts 204", () => {
        const requests: string[] = [];
        const layer = makeLayer((request) => {
            requests.push(request.method);
            return Effect.succeed(
                responseFromText({
                    url: request.url,
                    status: request.method === "HEAD" ? 404 : 204,
                }),
            );
        });
        return Effect.gen(function* () {
            const exists = yield* Exists;
            const deleteFile = yield* DeleteFile;
            expect(yield* exists.execute("missing.txt")).toBe(false);
            yield* deleteFile.execute("old.txt");
            expect(requests).toEqual(["HEAD", "DELETE"]);
        }).pipe(Effect.provide(layer));
    });

    it.effect("reads DAV compliance headers case-insensitively", () => {
        const headers: Array<Record<string, string>> = [
            { dav: "1, 2", server: "rshs" },
            { DAV: "1, 2", Server: "rshs" },
            { DaV: "1, 2", SeRvEr: "rshs" },
        ];
        let responseIndex = 0;
        const layer = makeLayer((request) =>
            Effect.succeed(
                responseFromText({
                    url: request.url,
                    headers: headers[responseIndex++] ?? headers[0],
                }),
            ),
        );
        return Effect.gen(function* () {
            const getDAVCompliance = yield* GetDavCompliance;
            for (const _ of headers) {
                expect(yield* getDAVCompliance.execute()).toEqual({
                    compliance: ["1", "2"],
                    server: "rshs",
                });
            }
        }).pipe(Effect.provide(layer));
    });

    it.effect("moves, copies, recursively creates, and unlocks resources", () => {
        const requests: Array<{ method: string; url: string; headers?: Record<string, string> }> =
            [];
        const layer = makeLayer((request) => {
            requests.push(request);
            const status =
                request.method === "MKCOL" ? 405 : request.method === "UNLOCK" ? 204 : 201;
            return Effect.succeed(responseFromText({ url: request.url, status }));
        });
        return Effect.gen(function* () {
            yield* (yield* MoveFile).execute("a", "b", { overwrite: false });
            yield* (yield* CopyFile).execute("a", "b", { shallow: true });
            yield* (yield* CreateDirectory).execute("one/two", { recursive: true });
            yield* (yield* Unlock).execute("b", "opaquelocktoken:1");
            expect(requests.map((request) => request.method)).toEqual([
                "MOVE",
                "COPY",
                "MKCOL",
                "MKCOL",
                "UNLOCK",
            ]);
            expect(requests[0]?.headers).toMatchObject({
                Destination: "https://dav.example.test/root/b",
                Overwrite: "F",
            });
            expect(requests[1]?.headers).toMatchObject({ Depth: "0", Overwrite: "T" });
            expect(requests[4]?.headers?.["Lock-Token"]).toBe("opaquelocktoken:1");
        }).pipe(Effect.provide(layer));
    });
});