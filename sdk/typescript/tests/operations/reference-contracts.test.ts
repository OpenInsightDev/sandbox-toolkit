import { Effect } from "effect";
import { describe, expect, it } from "@effect/vitest";
import {
    CopyFile,
    CreateDirectory,
    Exists,
    FileLinks,
    GetDirectoryContents,
    GetFileContents,
    Lock,
    PutFileContents,
    Stat,
} from "#/operations.ts";
import { responseFromText } from "#/transport.ts";
import { makeLayer, multistatus } from "./helpers.ts";

const lockBody =
    '<D:prop xmlns:D="DAV:"><D:lockdiscovery><D:activelock><D:locktoken><D:href>opaquelocktoken:abc</D:href></D:locktoken><D:timeout>Second-60</D:timeout></D:activelock></D:lockdiscovery></D:prop>';

describe("reference operation contracts", () => {
    it.effect("copies deeply by default and shallowly on request", () => {
        const requests: Array<{ headers?: Record<string, string> }> = [];
        const layer = makeLayer((request) => {
            requests.push(request);
            return Effect.succeed(responseFromText({ url: request.url, status: 201 }));
        });
        return Effect.gen(function* () {
            const copy = yield* CopyFile;
            yield* copy.execute("a", "b");
            yield* copy.execute("a", "b", { shallow: false });
            yield* copy.execute("a", "b", { shallow: true });
            expect(requests.map((request) => request.headers?.Depth)).toEqual([
                "infinity",
                "infinity",
                "0",
            ]);
            expect(requests[0]?.headers?.Destination).toBe("https://dav.example.test/root/b");
            expect(requests[0]?.headers?.Overwrite).toBe("T");
        }).pipe(Effect.provide(layer));
    });

    it.effect("adds a trailing slash to created directory requests", () => {
        const requests: string[] = [];
        const layer = makeLayer((request) => {
            requests.push(request.url);
            return Effect.succeed(responseFromText({ url: request.url, status: 201 }));
        });
        return Effect.gen(function* () {
            const createDirectory = yield* CreateDirectory;
            yield* createDirectory.execute("subdirectory-1122");
            yield* createDirectory.execute("a/b/c", { recursive: true });
            expect(requests).toEqual([
                "https://dav.example.test/root/subdirectory-1122/",
                "https://dav.example.test/root/a/",
                "https://dav.example.test/root/a/b/",
                "https://dav.example.test/root/a/b/c/",
            ]);
        }).pipe(Effect.provide(layer));
    });

    it.effect("reports existing resources and fails stat with the HTTP status", () => {
        const layer = makeLayer((request) =>
            Effect.succeed(
                responseFromText({
                    url: request.url,
                    status: request.method === "HEAD" ? 200 : 404,
                }),
            ),
        );
        return Effect.gen(function* () {
            const exists = yield* Exists;
            expect(yield* exists.execute("present.txt")).toBe(true);

            const stat = yield* Stat;
            const error = yield* Effect.flip(stat.execute("missing.txt"));
            expect(error).toMatchObject({
                _tag: "HttpStatusError",
                status: 404,
                operation: "stat",
            });
        }).pipe(Effect.provide(layer));
    });

    it.effect("passes custom headers through to the transport", () => {
        const requests: Array<{ headers?: Record<string, string> }> = [];
        const layer = makeLayer((request) => {
            requests.push(request);
            return Effect.succeed(responseFromText({ url: request.url, status: 200, body: "ok" }));
        });
        return Effect.gen(function* () {
            const getFileContents = yield* GetFileContents;
            yield* getFileContents.execute("format.json", {
                format: "text",
                headers: { "X-Test": "test" },
            });
            expect(requests[0]?.headers).toMatchObject({ "X-Test": "test" });
            expect(requests[0]?.headers?.Accept).toBe("text/plain");
        }).pipe(Effect.provide(layer));
    });

    it.effect("sends RFC-0001 line ranges and preserves response headers in details", () => {
        const requests: Array<{ headers?: Record<string, string> }> = [];
        const layer = makeLayer((request) => {
            requests.push(request);
            return Effect.succeed(
                responseFromText({
                    url: request.url,
                    status: 206,
                    body: "line two\\nline three",
                    headers: { "Content-Range": "lines 2-3/4", ETag: '"v1"' },
                }),
            );
        });
        return Effect.gen(function* () {
            const get = yield* GetFileContents;
            const first = yield* get.execute("notes.txt", {
                format: "text",
                lineRange: { start: 2, end: 3 },
                details: true,
            });
            const second = yield* get.execute("notes.txt", {
                format: "text",
                range: { unit: "lines", start: 2 },
            });

            expect(requests.map((request) => request.headers?.Range)).toEqual([
                "lines=2-3",
                "lines=2-",
            ]);
            expect(second).toBe("line two\\nline three");
            expect(first).toMatchObject({
                status: 206,
                body: new TextEncoder().encode("line two\\nline three"),
            });
            expect((first as { headers: Record<string, string> }).headers).toMatchObject({
                "Content-Range": "lines 2-3/4",
                ETag: '"v1"',
            });
        }).pipe(Effect.provide(layer));
    });

    it.effect("returns JSON payloads as text without parsing them", () => {
        const layer = makeLayer((request) =>
            Effect.succeed(
                responseFromText({
                    url: request.url,
                    headers: { "Content-Type": "application/json" },
                    body: '{"test":true}',
                }),
            ),
        );
        return Effect.gen(function* () {
            const getFileContents = yield* GetFileContents;
            const contents = yield* getFileContents.execute("format.json", { format: "text" });
            expect(contents).toBe('{"test":true}');
        }).pipe(Effect.provide(layer));
    });

    it.effect("honours the upload content length option", () => {
        const requests: Array<{ headers?: Record<string, string> }> = [];
        const layer = makeLayer((request) => {
            requests.push(request);
            return Effect.succeed(responseFromText({ url: request.url, status: 201 }));
        });
        return Effect.gen(function* () {
            const putFileContents = yield* PutFileContents;
            yield* putFileContents.execute("a.txt", "hello", { contentLength: 10 });
            yield* putFileContents.execute("b.txt", "hello", { contentLength: false });
            expect(requests[0]?.headers?.["Content-Length"]).toBe("10");
            expect(requests[1]?.headers).not.toHaveProperty("Content-Length");
        }).pipe(Effect.provide(layer));
    });

    it.effect("reports an overwrite conflict as false", () => {
        const layer = makeLayer((request) =>
            Effect.succeed(responseFromText({ url: request.url, status: 412 })),
        );
        return Effect.gen(function* () {
            const putFileContents = yield* PutFileContents;
            expect(yield* putFileContents.execute("a.txt", "hello", { overwrite: false })).toBe(
                false,
            );
        }).pipe(Effect.provide(layer));
    });

    it.effect("filters directory contents with a glob", () => {
        const layer = makeLayer((request) =>
            Effect.succeed(responseFromText({ url: request.url, status: 207, body: multistatus })),
        );
        return Effect.gen(function* () {
            const getDirectoryContents = yield* GetDirectoryContents;
            const items = yield* getDirectoryContents.execute("", { glob: "/root/*.txt" });
            expect(items.map((item) => item.basename)).toEqual(["notes one.txt"]);
        }).pipe(Effect.provide(layer));
    });

    it.effect("locks with a default and refreshed timeout", () => {
        const requests: Array<{ headers?: Record<string, string> }> = [];
        const layer = makeLayer((request) => {
            requests.push(request);
            return Effect.succeed(
                responseFromText({ url: request.url, status: 200, body: lockBody }),
            );
        });
        return Effect.gen(function* () {
            const lock = yield* Lock;
            expect(yield* lock.execute("notes.txt")).toMatchObject({
                token: "opaquelocktoken:abc",
                serverTimeout: "Second-60",
            });
            yield* lock.execute("notes.txt", { refreshToken: "opaquelocktoken:abc" });
            expect(requests[0]?.headers?.Timeout).toBe("Infinite, Second-4100000000");
            expect(requests[0]?.headers?.If).toBeUndefined();
            expect(requests[1]?.headers?.If).toBe("opaquelocktoken:abc");
        }).pipe(Effect.provide(layer));
    });

    it.effect("generates credential-free download and upload request descriptions", () => {
        const layer = makeLayer((request) =>
            Effect.succeed(responseFromText({ url: request.url, status: 204 })),
        );
        return Effect.gen(function* () {
            const links = yield* FileLinks;
            const download = yield* links.download("test/file.txt");
            expect(download).toEqual({
                url: "https://dav.example.test/root/test/file.txt",
                method: "GET",
                headers: {},
            });
            expect(download.url).not.toContain("@");

            const upload = yield* links.upload("test/file.txt");
            expect(upload).toEqual({
                url: "https://dav.example.test/root/test/file.txt",
                method: "PUT",
                headers: { "Content-Type": "application/octet-stream" },
            });
            expect(upload.url).not.toContain("@");
        }).pipe(Effect.provide(layer));
    });
});