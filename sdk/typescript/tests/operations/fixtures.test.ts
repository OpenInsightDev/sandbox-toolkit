import { Effect } from "effect";
import { describe, expect, it } from "@effect/vitest";
import { GetDirectoryContents, GetQuota, Search, Stat } from "#/operations.ts";
import { responseFromText } from "#/transport.ts";
import { readResponse } from "../fixtures/read.ts";
import { makeLayer } from "./helpers.ts";

const fixtureLayer = (name: string, status = 207) =>
    makeLayer((request) =>
        Effect.succeed(responseFromText({ url: request.url, status, body: readResponse(name) })),
    );

describe("migrated DAV response fixtures", () => {
    it.effect("maps a propstat 404 inside a 207 to a status error", () => {
        const layer = fixtureLayer("nginx-not-found");
        return Effect.gen(function* () {
            const stat = yield* Stat;
            const error = yield* Effect.flip(stat.execute("does-not-exist"));
            expect(error).toMatchObject({ _tag: "HttpStatusError", status: 404 });
        }).pipe(Effect.provide(layer));
    });

    it.effect("returns an empty array for an empty multistatus", () => {
        const layer = fixtureLayer("empty-multistatus");
        return Effect.gen(function* () {
            const contents = yield* GetDirectoryContents;
            expect(yield* contents.execute("")).toEqual([]);
        }).pipe(Effect.provide(layer));
    });

    it.effect("filters the collection itself and decodes percent-encoded basenames", () => {
        const layer = fixtureLayer("seafile-propfind");
        return Effect.gen(function* () {
            const contents = yield* GetDirectoryContents;
            const items = yield* contents.execute("");
            expect(items).toHaveLength(1);
            expect(items[0]).toMatchObject({
                basename: "Ma bibliothèque",
                type: "directory",
                size: 0,
                etag: "2920f985ebc6692632c7c3ab46b3919556239d37",
            });
        }).pipe(Effect.provide(layer));
    });

    it.effect("decodes HTML entities in hrefs", () => {
        const layer = fixtureLayer("propfind-href-html-entities");
        return Effect.gen(function* () {
            const contents = yield* GetDirectoryContents;
            const encoded = yield* contents.execute("files", { includeSelf: true });
            expect(encoded[0]?.basename).toBe("&amp;.md");
            expect(encoded[0]?.filename).toMatch(/\/files\/&amp;\.md$/);
        }).pipe(Effect.provide(layer));
    });

    it.effect("preserves query strings in decoded basenames", () => {
        const layer = fixtureLayer("propfind-href-with-query");
        return Effect.gen(function* () {
            const contents = yield* GetDirectoryContents;
            const items = yield* contents.execute("files", { includeSelf: true });
            expect(items[0]?.basename).toBe("some file?foo=1&bar=2");
        }).pipe(Effect.provide(layer));
    });

    it.effect("returns full search results", () => {
        const layer = fixtureLayer("search-full-success");
        return Effect.gen(function* () {
            const search = yield* Search;
            const result = yield* search.execute("/", { data: "<searchrequest />" });
            expect(result.truncated).toBe(false);
            expect(result.results.map((item) => item.basename)).toEqual([
                "first-file.md",
                "second file.txt",
            ]);
            expect(result.results[0]).toMatchObject({ mime: "text/markdown", size: 0 });
        }).pipe(Effect.provide(layer));
    });

    it.effect("detects truncated search results", () => {
        const layer = fixtureLayer("search-truncated");
        return Effect.gen(function* () {
            const search = yield* Search;
            const result = yield* search.execute("/");
            expect(result.truncated).toBe(true);
            expect(result.results.map((item) => item.basename)).toEqual(["first-file.md"]);
        }).pipe(Effect.provide(layer));
    });

    it.effect("translates quota sentinel values", () => {
        const layer = fixtureLayer("quota-valid");
        return Effect.gen(function* () {
            const quota = yield* GetQuota;
            expect(yield* quota.execute()).toEqual({ used: 6864755191, available: "unlimited" });
        }).pipe(Effect.provide(layer));
    });

    it.effect("returns null when quota properties are missing", () => {
        const layer = fixtureLayer("quota-invalid");
        return Effect.gen(function* () {
            const quota = yield* GetQuota;
            expect(yield* quota.execute()).toBeNull();
        }).pipe(Effect.provide(layer));
    });

    it.effect("targets the quota request at the requested path", () => {
        const requests: string[] = [];
        const layer = makeLayer((request) => {
            requests.push(request.url);
            return Effect.succeed(
                responseFromText({
                    url: request.url,
                    status: 207,
                    body: readResponse("quota-valid"),
                }),
            );
        });
        return Effect.gen(function* () {
            const quota = yield* GetQuota;
            yield* quota.execute({ path: "sub1" });
            expect(requests[0]).toBe("https://dav.example.test/root/sub1");
        }).pipe(Effect.provide(layer));
    });

    it.effect("exposes display names and detailed properties from stat", () => {
        const layer = fixtureLayer("propfind-numeric-displayname");
        return Effect.gen(function* () {
            const stat = yield* Stat;
            const item = yield* stat.execute("1/", { details: true });
            expect(item).toMatchObject({ type: "directory" });
            expect(item.props?.displayname).toBe("1");
        }).pipe(Effect.provide(layer));
    });
});