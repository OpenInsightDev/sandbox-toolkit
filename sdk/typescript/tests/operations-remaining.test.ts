import { Effect, Layer, Stream } from "effect";
import { describe, expect, it } from "@effect/vitest";
import { decodeWebDavConfigSync } from "#/config.ts";
import { AuthTest } from "#/auth.ts";
import { responseFromText, TestTransport } from "#/transport.ts";
import {
    OperationsLive,
    GetFileContents,
    PutFileContents,
    CreateReadStream,
    Lock,
    FileLinks,
} from "#/operations.ts";

const config = decodeWebDavConfigSync({ remoteUrl: "https://dav.example.test", authType: "none" });
const makeLayer = (execute: Parameters<typeof TestTransport>[0]["execute"]) =>
    Layer.mergeAll(OperationsLive({ config }), AuthTest(config), TestTransport({ execute }));

describe("remaining WebDAV operations", () => {
    it.effect("downloads text and uploads bytes with overwrite behavior", () => {
        const requests: Array<{
            method: string;
            headers?: Record<string, string>;
            data?: unknown;
        }> = [];
        const layer = makeLayer((request) => {
            requests.push(request);
            return request.method === "GET"
                ? Effect.succeed(responseFromText({ url: request.url, body: "hello" }))
                : Effect.succeed(responseFromText({ url: request.url, status: 201 }));
        });
        return Effect.gen(function* () {
            const downloader = yield* GetFileContents;
            const uploader = yield* PutFileContents;
            expect(yield* downloader.execute("a.txt", { format: "text" })).toBe("hello");
            expect(
                yield* uploader.execute("a.txt", new Uint8Array([1, 2]), { overwrite: false }),
            ).toBe(true);
            expect(requests[1]?.headers?.["If-None-Match"]).toBe("*");
            expect(requests[1]?.headers?.["Content-Length"]).toBe("2");
        }).pipe(Effect.provide(layer));
    });

    it.effect("sets range and requires partial content", () => {
        const layer = makeLayer((request) =>
            Effect.succeed(responseFromText({ url: request.url, status: 206, body: "part" })),
        );
        return Effect.gen(function* () {
            const reader = yield* CreateReadStream;
            const stream = yield* reader.execute("a.txt", { range: { start: 2, end: 4 } });
            expect(yield* Stream.runCollect(stream)).toHaveLength(1);
        }).pipe(Effect.provide(layer));
    });

    it.effect("parses lock token and generates credential-free links", () => {
        const layer = makeLayer((request) =>
            Effect.succeed(
                responseFromText({
                    url: request.url,
                    body: '<D:prop xmlns:D="DAV:"><D:lockdiscovery><D:activelock><D:locktoken><D:href>opaquelocktoken:1</D:href></D:locktoken><D:timeout>Second-60</D:timeout></D:activelock></D:lockdiscovery></D:prop>',
                }),
            ),
        );
        return Effect.gen(function* () {
            const locker = yield* Lock;
            const links = yield* FileLinks;
            const result = yield* locker.execute("a.txt");
            expect(result.token).toBe("opaquelocktoken:1");
            expect((yield* links.download("a.txt")).url).not.toContain("@");
        }).pipe(Effect.provide(layer));
    });
});