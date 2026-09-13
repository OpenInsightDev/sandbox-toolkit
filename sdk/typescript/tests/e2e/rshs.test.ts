import fs from "node:fs";
import path from "node:path";
import { Effect, Stream } from "effect";
import { afterAll, beforeAll, beforeEach, describe, expect, it } from "@effect/vitest";
import {
    SERVER_CONTENTS,
    baseUrl,
    captureRequests,
    collectBytes,
    resetFixtures,
    startServer,
    stopServer,
    withClient,
} from "./harness.ts";

/**
 * End-to-end tests migrated from the reference `webdav` client's real-server
 * suites (`references/webdav-client/test/node/...`). They talk to an actual
 * `rshs` container instead of the reference's in-process `webdav-server`, and
 * are rewritten against the Effect service API.
 *
 * Server- or feature-specific suites from the reference are intentionally not
 * migrated here (documented in the accompanying report): Digest auth (rshs
 * only supports Basic), in-process `details` responses, SEARCH, quota, and
 * `partialUpdateFileContents`.
 */
describe.runIf(process.env.WEBDAV_E2E === "1")("rshs WebDAV server (end-to-end)", () => {
    beforeAll(async () => {
        await startServer();
    }, 120_000);

    afterAll(() => {
        stopServer();
    });

    beforeEach(() => {
        resetFixtures();
    });

    describe("stat", () => {
        it.effect("stats files with size, mime, and type", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    const stat = yield* client.stat("/alrighty.jpg");
                    expect(stat).toMatchObject({
                        filename: "/alrighty.jpg",
                        basename: "alrighty.jpg",
                        type: "file",
                        size: 52130,
                        mime: "image/jpeg",
                    });
                    expect(typeof stat.lastmod).toBe("string");
                    expect(stat.lastmod.length).toBeGreaterThan(0);
                }),
            ),
        );

        it.effect("stats files with '%' in the path", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    const stat = yield* client.stat("/file % name.txt");
                    expect(stat).toMatchObject({
                        filename: "/file % name.txt",
                        basename: "file % name.txt",
                    });
                }),
            ),
        );

        it.effect("stats directories with '%' in the path", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    const stat = yield* client.stat("/two%20words");
                    expect(stat).toMatchObject({
                        filename: "/two%20words",
                        basename: "two%20words",
                    });
                }),
            ),
        );

        it.effect("stats directories", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    const stat = yield* client.stat("/webdav/server");
                    expect(stat).toMatchObject({
                        filename: "/webdav/server",
                        basename: "server",
                        type: "directory",
                        size: 0,
                    });
                }),
            ),
        );

        it.effect("stats the root", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    const stat = yield* client.stat("/");
                    expect(stat).toMatchObject({
                        filename: "/",
                        basename: "",
                        type: "directory",
                        size: 0,
                    });
                }),
            ),
        );

        it.effect("fails with 404 on a non-existent file", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    const error = yield* Effect.flip(client.stat("/does-not-exist"));
                    expect(error).toMatchObject({ _tag: "HttpStatusError", status: 404 });
                }),
            ),
        );
    });

    describe("authentication", () => {
        it.effect("connects using HTTP Basic when credentials are provided", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    const stat = yield* client.stat("/notes.txt");
                    expect(stat.basename).toBe("notes.txt");
                }),
            ),
        );

        it.effect("supports auto-detection of password authentication", () =>
            withClient(
                (client) =>
                    Effect.gen(function* () {
                        expect(yield* client.exists("/notes.txt")).toBe(true);
                    }),
                { authType: "auto" },
            ),
        );

        it.effect("fails for anonymous requests against an authenticated server", () =>
            withClient(
                (client) =>
                    Effect.gen(function* () {
                        const error = yield* Effect.flip(client.stat("/notes.txt"));
                        expect(error).toMatchObject({ _tag: "AuthenticationError" });
                    }),
                { authType: "none", username: undefined, password: undefined },
            ),
        );

        it.effect("fails for incorrect credentials", () =>
            withClient(
                (client) =>
                    Effect.gen(function* () {
                        const error = yield* Effect.flip(client.stat("/notes.txt"));
                        expect(error).toMatchObject({ _tag: "AuthenticationError" });
                    }),
                { password: "wrong-password" },
            ),
        );

        it.effect("rejects Bearer tokens because rshs only supports Basic", () =>
            withClient(
                (client) =>
                    Effect.gen(function* () {
                        const error = yield* Effect.flip(client.stat("/notes.txt"));
                        expect(error).toMatchObject({ _tag: "AuthenticationError" });
                    }),
                {
                    authType: "token",
                    token: { token_type: "Bearer", access_token: "ABC123" },
                    username: undefined,
                    password: undefined,
                },
            ),
        );
    });

    describe("getDirectoryContents", () => {
        it.effect("returns an array of items", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    const contents = yield* client.getDirectoryContents("/");
                    expect(Array.isArray(contents)).toBe(true);
                    expect(contents.length).toBeGreaterThan(0);
                }),
            ),
        );

        it.effect("returns correct directory results", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    const contents = yield* client.getDirectoryContents("/");
                    const sub1 = contents.find((item) => item.basename === "sub1");
                    expect(sub1).toMatchObject({ filename: "/sub1", size: 0, type: "directory" });
                }),
            ),
        );

        it.effect("does not include the base directory by default", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    const contents = yield* client.getDirectoryContents("/sub1");
                    expect(contents.find((item) => item.basename === "sub1")).toBeUndefined();
                }),
            ),
        );

        it.effect("returns only expected results when using a trailing slash", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    const contents = yield* client.getDirectoryContents("/webdav/");
                    expect(contents.map((item) => item.filename)).toEqual(["/webdav/server"]);
                }),
            ),
        );

        it.effect("returns correct file results", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    const contents = yield* client.getDirectoryContents("/");
                    const jpg = contents.find((item) => item.basename === "alrighty.jpg");
                    const amp = contents.find((item) => item.basename === "file&name.txt");
                    expect(jpg).toMatchObject({
                        filename: "/alrighty.jpg",
                        size: 52130,
                        type: "file",
                    });
                    expect(amp?.filename).toBe("/file&name.txt");
                }),
            ),
        );

        it.effect("returns correct file results in a sub-directory", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    const contents = yield* client.getDirectoryContents("/sub1");
                    const jpg = contents.find((item) => item.basename === "irrelephant.jpg");
                    expect(jpg).toMatchObject({
                        filename: "/sub1/irrelephant.jpg",
                        size: 138008,
                        type: "file",
                    });
                }),
            ),
        );

        it.effect("accepts a path without a leading slash", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    const contents = yield* client.getDirectoryContents("sub1");
                    expect(contents).toHaveLength(2);
                    expect(
                        contents.find((item) => item.basename === "irrelephant.jpg"),
                    ).toBeDefined();
                    expect(contents.find((item) => item.basename === "ยากจน #1.txt")).toBeDefined();
                }),
            ),
        );

        it.effect("returns correct results for names with special characters", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    const contents = yield* client.getDirectoryContents("/sub1");
                    const thai = contents.find((item) => item.basename === "ยากจน #1.txt");
                    expect(thai?.filename).toBe("/sub1/ยากจน #1.txt");
                }),
            ),
        );

        it.effect("returns the contents of a directory with repetitive naming", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    const contents = yield* client.getDirectoryContents("/webdav/server");
                    expect(contents[0]).toMatchObject({ basename: "notreal.txt" });
                }),
            ),
        );

        it.effect("returns only the directory contents for a path with spaces (issue #68)", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    const contents = yield* client.getDirectoryContents("/two words");
                    expect(contents).toHaveLength(1);
                    expect(contents[0]?.basename).toBe("file.txt");
                }),
            ),
        );

        it.effect("returns only the directory contents for a directory with '&' in the name", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    const contents = yield* client.getDirectoryContents("/with & in path");
                    expect(contents).toHaveLength(1);
                    expect(contents[0]?.basename).toBe("file.txt");
                }),
            ),
        );

        it.effect(
            "returns correct contents when the path contains encoded sequences (issue #93)",
            () =>
                withClient((client) =>
                    Effect.gen(function* () {
                        const contents = yield* client.getDirectoryContents("/two%20words");
                        expect(contents).toHaveLength(1);
                        expect(contents[0]?.basename).toBe("file2.txt");
                    }),
                ),
        );

        it.effect("distinguishes directories that contain '%' in their name", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    const contents = yield* client.getDirectoryContents("/");
                    expect(contents.find((item) => item.basename === "two words")).toMatchObject({
                        type: "directory",
                    });
                    expect(contents.find((item) => item.basename === "two%20words")).toMatchObject({
                        type: "directory",
                    });
                }),
            ),
        );

        it.effect("supports globbing files", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    const contents = yield* client.getDirectoryContents("/", {
                        deep: true,
                        glob: "/webdav/**/*.txt",
                    });
                    expect(contents).toHaveLength(1);
                    expect(contents[0]?.filename).toBe("/webdav/server/notreal.txt");
                }),
            ),
        );

        it.effect("includes the directory itself when includeSelf is enabled", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    const contents = yield* client.getDirectoryContents("/", { includeSelf: true });
                    const root = contents.find((item) => item.basename === "");
                    expect(root).toMatchObject({ filename: "/", size: 0, type: "directory" });
                    expect(contents).toHaveLength(12);
                }),
            ),
        );

        it.effect("includes the sub-directory itself when includeSelf is enabled", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    const contents = yield* client.getDirectoryContents("/sub1", {
                        includeSelf: true,
                    });
                    const sub1 = contents.find((item) => item.basename === "sub1");
                    expect(sub1).toMatchObject({ filename: "/sub1", size: 0, type: "directory" });
                    expect(contents).toHaveLength(3);
                }),
            ),
        );

        it.effect("forwards custom headers", () => {
            const spy = captureRequests();
            return withClient((client) =>
                Effect.gen(function* () {
                    yield* client.getDirectoryContents("/", { headers: { "X-test": "test" } });
                }),
            ).pipe(
                Effect.ensuring(Effect.sync(spy.restore)),
                Effect.tap(() =>
                    Effect.sync(() => {
                        expect(spy.requests[0]?.headers["x-test"]).toBe("test");
                    }),
                ),
            );
        });
    });

    describe("exists", () => {
        it.effect("detects existing files", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    expect(yield* client.exists("/two%20words/file2.txt")).toBe(true);
                }),
            ),
        );

        it.effect("detects existing directories", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    expect(yield* client.exists("/webdav/server")).toBe(true);
                }),
            ),
        );

        it.effect("reports false for non-existing paths", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    expect(yield* client.exists("/webdav/this/is/not/here.txt")).toBe(false);
                }),
            ),
        );

        it.effect("forwards custom headers", () => {
            const spy = captureRequests();
            return withClient((client) =>
                Effect.gen(function* () {
                    yield* client.exists("/test.txt", { headers: { "X-test": "test" } });
                }),
            ).pipe(
                Effect.ensuring(Effect.sync(spy.restore)),
                Effect.tap(() =>
                    Effect.sync(() => {
                        expect(spy.requests[0]?.headers["x-test"]).toBe("test");
                    }),
                ),
            );
        });
    });

    describe("getFileContents", () => {
        it.effect("reads a remote file into a byte array", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    const remote = yield* client.getFileContents("/alrighty.jpg");
                    const local = fs.readFileSync(path.join(SERVER_CONTENTS, "alrighty.jpg"));
                    expect(Buffer.from(remote as Uint8Array).equals(local)).toBe(true);
                }),
            ),
        );

        it.effect("returns detailed results as bytes plus metadata", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    const details = (yield* client.getFileContents("/alrighty.jpg", {
                        details: true,
                    })) as { body: Uint8Array; headers: Record<string, string> };
                    expect(details.body).toBeInstanceOf(Uint8Array);
                    expect(typeof details.headers).toBe("object");
                }),
            ),
        );

        it.effect("reads a remote file into a string", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    const remote = yield* client.getFileContents("/text document.txt", {
                        format: "text",
                    });
                    const local = fs.readFileSync(
                        path.join(SERVER_CONTENTS, "text document.txt"),
                        "utf8",
                    );
                    expect(remote).toBe(local);
                }),
            ),
        );

        it.effect("retrieves JSON files as text (issue #267)", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    const contents = yield* client.getFileContents("/format.json", {
                        format: "text",
                    });
                    expect(contents).toContain('{"test":true}');
                }),
            ),
        );

        it.effect("forwards custom headers", () => {
            const spy = captureRequests();
            return withClient((client) =>
                Effect.gen(function* () {
                    yield* client.getFileContents("/text document.txt", {
                        format: "text",
                        headers: { "X-test": "test" },
                    });
                }),
            ).pipe(
                Effect.ensuring(Effect.sync(spy.restore)),
                Effect.tap(() =>
                    Effect.sync(() => {
                        expect(spy.requests[0]?.headers["x-test"]).toBe("test");
                    }),
                ),
            );
        });
    });

    describe("putFileContents", () => {
        it.effect("writes binary files", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    const source = fs.readFileSync(path.join(SERVER_CONTENTS, "alrighty.jpg"));
                    yield* client.putFileContents("/sub1/alrighty.jpg", source);
                    const written = (yield* client.getFileContents(
                        "/sub1/alrighty.jpg",
                    )) as Uint8Array;
                    expect(Buffer.from(written).equals(source)).toBe(true);
                }),
            ),
        );

        it.effect("writes text files", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    const text = "this is\nsome text\ncontent\t...\n";
                    yield* client.putFileContents("/newFile.txt", text);
                    expect(yield* client.getFileContents("/newFile.txt", { format: "text" })).toBe(
                        text,
                    );
                }),
            ),
        );

        it.effect("writes streams", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    const source = fs.readFileSync(path.join(SERVER_CONTENTS, "alrighty.jpg"));
                    yield* client.putFileContents("/sub1/alrighty.jpg", Stream.make(source));
                    const written = (yield* client.getFileContents(
                        "/sub1/alrighty.jpg",
                    )) as Uint8Array;
                    expect(Buffer.from(written).equals(source)).toBe(true);
                }),
            ),
        );

        it.effect("writes files with non-latin characters in the filename", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    const text = "this is\nsome text\ncontent\t...\n";
                    yield* client.putFileContents("/จะทำลาย.txt", text);
                    expect(yield* client.getFileContents("/จะทำลาย.txt", { format: "text" })).toBe(
                        text,
                    );
                }),
            ),
        );

        it.effect(
            "does not protect existing files when overwrite is disabled (rshs limitation)",
            () =>
                withClient((client) =>
                    Effect.gen(function* () {
                        // rshs does not implement `If-None-Match: *`, so a conditional PUT is
                        // treated as a plain overwrite. This test pins the observed behaviour
                        // so the data-loss risk is captured instead of silently passing.
                        const result = yield* client.putFileContents("/notes.txt", "replaced", {
                            overwrite: false,
                        });
                        expect(result).toBe(true);
                        expect(
                            yield* client.getFileContents("/notes.txt", { format: "text" }),
                        ).toBe("replaced");
                    }),
                ),
        );
    });

    describe("createDirectory", () => {
        it.effect("creates directories", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    expect(yield* client.exists("/sub2")).toBe(false);
                    yield* client.createDirectory("/sub2");
                    expect((yield* client.stat("/sub2")).type).toBe("directory");
                }),
            ),
        );

        it.effect("supports creating deep directories", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    expect(yield* client.exists("/a/b/c/d/e")).toBe(false);
                    yield* client.createDirectory("/a/b/c/d/e", { recursive: true });
                    expect((yield* client.stat("/a/b/c/d/e")).type).toBe("directory");
                }),
            ),
        );

        it.effect("supports creating deep directories which partially exist", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    expect(yield* client.exists("/sub1/a/b")).toBe(false);
                    yield* client.createDirectory("/sub1/a/b", { recursive: true });
                    expect((yield* client.stat("/sub1/a/b")).type).toBe("directory");
                }),
            ),
        );

        it.effect("adds a trailing slash to the directory request", () => {
            const spy = captureRequests();
            return withClient((client) =>
                Effect.gen(function* () {
                    yield* client.createDirectory("/subdirectory-1122");
                }),
            ).pipe(
                Effect.ensuring(Effect.sync(spy.restore)),
                Effect.tap(() =>
                    Effect.sync(() => {
                        expect(spy.requests[0]?.url.endsWith("/")).toBe(true);
                    }),
                ),
            );
        });
    });

    describe("deleteFile", () => {
        it.effect("deletes a remote file", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    expect(yield* client.exists("/text document.txt")).toBe(true);
                    yield* client.deleteFile("/text document.txt");
                    expect(yield* client.exists("/text document.txt")).toBe(false);
                }),
            ),
        );

        it.effect("deletes a remote directory", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    expect(yield* client.exists("/sub1")).toBe(true);
                    yield* client.deleteFile("/sub1");
                    expect(yield* client.exists("/sub1")).toBe(false);
                }),
            ),
        );

        it.effect("forwards custom headers", () => {
            const spy = captureRequests();
            return withClient((client) =>
                Effect.gen(function* () {
                    yield* client.deleteFile("/text document.txt", {
                        headers: { "X-test": "test" },
                    });
                }),
            ).pipe(
                Effect.ensuring(Effect.sync(spy.restore)),
                Effect.tap(() =>
                    Effect.sync(() => {
                        expect(spy.requests[0]?.headers["x-test"]).toBe("test");
                    }),
                ),
            );
        });
    });

    describe("moveFile", () => {
        it.effect("moves files from one directory to another", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    yield* client.moveFile("/alrighty.jpg", "/sub1/alrighty.jpg");
                    expect(yield* client.exists("/alrighty.jpg")).toBe(false);
                    expect(yield* client.exists("/sub1/alrighty.jpg")).toBe(true);
                }),
            ),
        );

        it.effect("moves directories from one directory to another", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    yield* client.moveFile("/webdav", "/sub1/webdav");
                    expect(yield* client.exists("/webdav")).toBe(false);
                    expect(yield* client.exists("/sub1/webdav")).toBe(true);
                }),
            ),
        );

        it.effect("moves files from one name to another", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    yield* client.moveFile("/alrighty.jpg", "/renamed.jpg");
                    expect(yield* client.exists("/alrighty.jpg")).toBe(false);
                    expect(yield* client.exists("/renamed.jpg")).toBe(true);
                }),
            ),
        );

        it.effect("overwrites on move by default", () => {
            const spy = captureRequests();
            return withClient((client) =>
                Effect.gen(function* () {
                    yield* client.moveFile("/two words/file.txt", "/with & in path/file.txt");
                }),
            ).pipe(
                Effect.ensuring(Effect.sync(spy.restore)),
                Effect.tap(() =>
                    Effect.sync(() => {
                        expect(spy.requests[0]?.headers["overwrite"]).toBe("T");
                    }),
                ),
            );
        });

        it.effect("fails with 412 when overwrite is disabled and the target exists", () => {
            const spy = captureRequests();
            return withClient((client) =>
                Effect.gen(function* () {
                    const error = yield* Effect.flip(
                        client.moveFile("/two words/file.txt", "/with & in path/file.txt", {
                            overwrite: false,
                        }),
                    );
                    expect(error).toMatchObject({ _tag: "HttpStatusError", status: 412 });
                }),
            ).pipe(
                Effect.ensuring(Effect.sync(spy.restore)),
                Effect.tap(() =>
                    Effect.sync(() => {
                        expect(spy.requests[0]?.headers["overwrite"]).toBe("F");
                    }),
                ),
            );
        });
    });

    describe("copyFile", () => {
        it.effect("copies files from one directory to another", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    yield* client.copyFile("/alrighty.jpg", "/sub1/alrighty.jpg");
                    expect(yield* client.exists("/alrighty.jpg")).toBe(true);
                    expect(yield* client.exists("/sub1/alrighty.jpg")).toBe(true);
                }),
            ),
        );

        it.effect("copies directories from one directory to another", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    yield* client.copyFile("/webdav", "/sub1/webdav");
                    expect((yield* client.stat("/webdav")).type).toBe("directory");
                    expect((yield* client.stat("/sub1/webdav")).type).toBe("directory");
                }),
            ),
        );

        it.effect("copies files with special characters", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    yield* client.copyFile("/sub1/ยากจน #1.txt", "/sub1/ยากจน #2.txt");
                    expect(yield* client.exists("/sub1/ยากจน #2.txt")).toBe(true);
                }),
            ),
        );

        it.effect("creates a deep copy by default", () => {
            const spy = captureRequests();
            return withClient((client) =>
                Effect.gen(function* () {
                    yield* client.copyFile("/alrighty.jpg", "/sub1/alrighty.jpg");
                }),
            ).pipe(
                Effect.ensuring(Effect.sync(spy.restore)),
                Effect.tap(() =>
                    Effect.sync(() => {
                        expect(spy.requests[0]?.headers["depth"]).toBe("infinity");
                    }),
                ),
            );
        });

        it.effect("creates a shallow copy when enabled", () => {
            const spy = captureRequests();
            return withClient((client) =>
                Effect.gen(function* () {
                    yield* client.copyFile("/alrighty.jpg", "/sub1/alrighty.jpg", {
                        shallow: true,
                    });
                }),
            ).pipe(
                Effect.ensuring(Effect.sync(spy.restore)),
                Effect.tap(() =>
                    Effect.sync(() => {
                        expect(spy.requests[0]?.headers["depth"]).toBe("0");
                    }),
                ),
            );
        });
    });

    describe("createReadStream", () => {
        it.effect("streams the entire contents of a remote file", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    const stream = yield* client.createReadStream("/alrighty.jpg");
                    const bytes = yield* collectBytes(stream);
                    const local = fs.readFileSync(path.join(SERVER_CONTENTS, "alrighty.jpg"));
                    expect(bytes.length).toBe(52130);
                    expect(Buffer.from(bytes).equals(local)).toBe(true);
                }),
            ),
        );

        it.effect("fails when the server ignores the Range header (rshs limitation)", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    // rshs answers `Range` requests with `200 OK` and the full body instead
                    // of `206 Partial Content`; the client rejects the un-honoured range.
                    const error = yield* Effect.flip(
                        client.createReadStream("/alrighty.jpg", {
                            range: { start: 0, end: 24999 },
                        }),
                    );
                    expect(error).toMatchObject({ _tag: "HttpStatusError", status: 200 });
                }),
            ),
        );
    });

    describe("createWriteStream", () => {
        it.effect("writes the file to the remote", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    const source = fs.readFileSync(path.join(SERVER_CONTENTS, "alrighty.jpg"));
                    yield* client.createWriteStream("/alrighty2.jpg", Stream.make(source));
                    const written = (yield* client.getFileContents("/alrighty2.jpg")) as Uint8Array;
                    expect(Buffer.from(written).equals(source)).toBe(true);
                }),
            ),
        );
    });

    describe("lock / unlock", () => {
        const target = "/notes.txt";

        it.effect("locks files and returns a token", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    const lock = yield* client.lock(target);
                    expect(lock.token).toMatch(/^[a-z0-9]+:.+/i);
                    yield* client.unlock(target, lock.token);
                }),
            ),
        );

        it.effect("supports unlocking", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    const lock = yield* client.lock(target);
                    yield* client.unlock(target, lock.token);
                }),
            ),
        );

        it.effect("fails unlocking with an invalid token", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    const lock = yield* client.lock(target);
                    const error = yield* Effect.flip(client.unlock(target, `${lock.token}z`));
                    expect(error).toMatchObject({ _tag: "HttpStatusError", status: 403 });
                    yield* client.unlock(target, lock.token);
                }),
            ),
        );
    });

    describe("customRequest", () => {
        it.effect("sends a PROPFIND request and returns the DAV payload", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    const response = yield* client.customRequest({
                        url: new URL("/alrighty.jpg", baseUrl()).toString(),
                        method: "PROPFIND",
                        headers: { Accept: "text/plain,application/xml", Depth: "0" },
                    });
                    expect(response.status).toBe(207);
                    expect(response.body).toContain("alrighty.jpg");
                    expect(response.body).toContain("image/jpeg");
                    expect(response.body).toContain("52130");
                }),
            ),
        );
    });

    describe("getDAVCompliance", () => {
        it.effect("reports the DAV compliance classes advertised by the server", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    const compliance = yield* client.getDAVCompliance("/");
                    expect(compliance.compliance).toEqual(["1", "2"]);
                }),
            ),
        );
    });

    describe("getQuota", () => {
        it.effect("returns null when the server does not expose quota properties", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    expect(yield* client.getQuota()).toBeNull();
                }),
            ),
        );
    });

    describe("file links", () => {
        it.effect("generates credential-free download and upload request descriptions", () =>
            withClient((client) =>
                Effect.gen(function* () {
                    const download = yield* client.getFileDownloadLink("/test/file.txt");
                    expect(download).toEqual({
                        url: new URL("/test/file.txt", baseUrl()).toString(),
                        method: "GET",
                        headers: {},
                    });
                    expect(download.url).not.toContain("@");

                    const upload = yield* client.getFileUploadLink("/test/file.txt");
                    expect(upload).toEqual({
                        url: new URL("/test/file.txt", baseUrl()).toString(),
                        method: "PUT",
                        headers: { "Content-Type": "application/octet-stream" },
                    });
                    expect(upload.url).not.toContain("@");
                }),
            ),
        );
    });
});