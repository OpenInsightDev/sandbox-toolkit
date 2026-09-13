import { Effect } from "effect";
import { describe, expect, it } from "@effect/vitest";
import { FileLinks, Lock, Unlock } from "#/operations.ts";
import { responseFromText } from "#/transport.ts";
import { makeLayer } from "./helpers.ts";

describe("WebDAV lock operations", () => {
    it.effect("creates and releases locks and provides download/upload links", () => {
        const requests: Array<{
            method: string;
            headers?: Record<string, string>;
            data?: unknown;
        }> = [];
        const layer = makeLayer((request) => {
            requests.push(request);
            return Effect.succeed(
                responseFromText({
                    url: request.url,
                    status: request.method === "LOCK" ? 200 : 204,
                    body:
                        request.method === "LOCK"
                            ? '<D:prop xmlns:D="DAV:"><D:lockdiscovery><D:activelock><D:locktoken><D:href>opaquelocktoken:abc</D:href></D:locktoken><D:timeout>Second-60</D:timeout></D:activelock></D:lockdiscovery></D:prop>'
                            : "",
                }),
            );
        });
        return Effect.gen(function* () {
            const lock = yield* Lock;
            const unlock = yield* Unlock;
            const links = yield* FileLinks;
            expect(yield* lock.execute("a", { timeout: "Second-60" })).toEqual({
                token: "opaquelocktoken:abc",
                serverTimeout: "Second-60",
            });
            yield* unlock.execute("a", "opaquelocktoken:abc");
            expect(yield* links.download("a")).toEqual({
                url: "https://dav.example.test/root/a",
                method: "GET",
                headers: {},
            });
            expect(yield* links.upload("a")).toMatchObject({
                method: "PUT",
                headers: { "Content-Type": "application/octet-stream" },
            });
            expect(requests[0]?.headers).toMatchObject({ Timeout: "Second-60" });
            expect(String(requests[0]?.data)).toContain("lockinfo");
        }).pipe(Effect.provide(layer));
    });
});