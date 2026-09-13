import { Effect } from "effect";
import { describe, expect, it } from "@effect/vitest";
import { GetDavCompliance, GetDirectoryContents, GetQuota, Search, Stat } from "#/operations.ts";
import { makeLayer, davResponse, fileMultistatus, multistatus } from "./helpers.ts";

describe("WebDAV query operations", () => {
    it.effect("parses stat and directory contents", () => {
        const layer = makeLayer((request) =>
            Effect.succeed(
                davResponse(
                    request.url,
                    request.url.endsWith("notes%20one.txt") ? fileMultistatus : multistatus,
                ),
            ),
        );
        return Effect.gen(function* () {
            const stat = yield* Stat;
            const contents = yield* GetDirectoryContents;
            const item = yield* stat.execute("notes one.txt");
            expect(item).toMatchObject({
                basename: "notes one.txt",
                size: 12,
                type: "file",
                etag: '"abc"',
                mime: "text/plain",
            });
            expect(yield* contents.execute("", { includeSelf: false })).toHaveLength(1);
            expect(yield* contents.execute("", { includeSelf: true })).toHaveLength(2);
        }).pipe(Effect.provide(layer));
    });

    it.effect("returns search results and DAV capabilities", () => {
        const requests: Array<{ method: string; url: string; headers?: Record<string, string> }> =
            [];
        const layer = makeLayer((request) => {
            requests.push(request);
            return request.method === "OPTIONS"
                ? Effect.succeed({
                      ...davResponse(request.url, "", 200),
                      headers: { DAV: "1, 2", Server: "dav-test" },
                  })
                : Effect.succeed(davResponse(request.url, multistatus));
        });
        return Effect.gen(function* () {
            const search = yield* Search;
            const compliance = yield* GetDavCompliance;
            const result = yield* search.execute("", { data: "<searchrequest />" });
            // Search returns every response that carries a propstat, including the arbiter.
            expect(result).toMatchObject({
                truncated: false,
                results: [
                    expect.objectContaining({ basename: "root", type: "directory" }),
                    expect.objectContaining({ basename: "notes one.txt" }),
                ],
            });
            expect(requests[0]).toMatchObject({ method: "SEARCH" });
            expect(requests[0]?.headers?.["Content-Type"]).toContain("application/xml");
            expect(yield* compliance.execute()).toEqual({
                compliance: ["1", "2"],
                server: "dav-test",
            });
        }).pipe(Effect.provide(layer));
    });

    it.effect("parses quota values and unknown quota responses", () => {
        const xml = `<D:multistatus xmlns:D="DAV:"><D:response><D:href>/root</D:href><D:propstat><D:prop><D:quota-used-bytes>10</D:quota-used-bytes><D:quota-available-bytes>90</D:quota-available-bytes></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response></D:multistatus>`;
        const layer = makeLayer((request) => Effect.succeed(davResponse(request.url, xml)));
        return Effect.gen(function* () {
            const quota = yield* GetQuota;
            expect(yield* quota.execute()).toEqual({ used: 10, available: 90 });
        }).pipe(Effect.provide(layer));
    });
});