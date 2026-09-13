import { Effect, Stream } from "effect";
import { describe, expect, it } from "@effect/vitest";
import {
    CreateReadStream,
    CreateWriteStream,
    GetFileContents,
    PartialUpdateFileContents,
    PutFileContents,
} from "#/operations.ts";
import { responseFromBytes, responseFromText } from "#/transport.ts";
import { makeLayer } from "./helpers.ts";

describe("WebDAV file and stream operations", () => {
    it.effect("downloads detailed bytes and uploads strings, bytes, and streams", () => {
        const requests: Array<{
            method: string;
            headers?: Record<string, string>;
            data?: unknown;
        }> = [];
        const layer = makeLayer((request) => {
            requests.push(request);
            return request.method === "GET"
                ? Effect.succeed(
                      responseFromBytes({
                          url: request.url,
                          body: new Uint8Array([1, 2, 3]),
                          headers: { ETag: "x" },
                      }),
                  )
                : Effect.succeed(responseFromText({ url: request.url, status: 201 }));
        });
        return Effect.gen(function* () {
            const get = yield* GetFileContents;
            const put = yield* PutFileContents;
            const partial = yield* PartialUpdateFileContents;
            const write = yield* CreateWriteStream;
            expect(yield* get.execute("a", { details: true })).toMatchObject({
                body: new Uint8Array([1, 2, 3]),
            });
            expect(yield* put.execute("a", "hello")).toBe(true);
            expect(yield* put.execute("b", new Uint8Array([1]), { overwrite: false })).toBe(true);
            expect(yield* partial.execute("c", new ArrayBuffer(2))).toBe(true);
            expect(yield* write.execute("d", Stream.fromIterable([new Uint8Array([7, 8])]))).toBe(
                true,
            );
            expect(requests[1]?.headers?.["Content-Length"]).toBe("5");
            expect(requests[2]?.headers?.["If-None-Match"]).toBe("*");
        }).pipe(Effect.provide(layer));
    });

    it.effect("sets byte ranges and requires 206 responses", () => {
        const requests: Array<{ headers?: Record<string, string> }> = [];
        const layer = makeLayer((request) => {
            requests.push(request);
            return Effect.succeed(
                responseFromBytes({ url: request.url, status: 206, body: new Uint8Array([1]) }),
            );
        });
        return Effect.gen(function* () {
            const read = yield* CreateReadStream;
            expect(
                yield* Stream.runCollect(yield* read.execute("a", { range: { start: 2, end: 4 } })),
            ).toHaveLength(1);
            expect(requests[0]?.headers?.Range).toBe("bytes=2-4");
        }).pipe(Effect.provide(layer));
    });

    it.effect("supports RFC-0001 line ranges for streaming reads", () => {
        const requests: Array<{ headers?: Record<string, string> }> = [];
        const layer = makeLayer((request) => {
            requests.push(request);
            return Effect.succeed(
                responseFromBytes({
                    url: request.url,
                    status: 206,
                    body: new Uint8Array([1, 2]),
                }),
            );
        });
        return Effect.gen(function* () {
            const read = yield* CreateReadStream;
            yield* Stream.runCollect(yield* read.execute("a", { lineRange: { start: 2 } }));
            yield* Stream.runCollect(
                yield* read.execute("a", { range: { unit: "lines", start: 3, end: 4 } }),
            );
            expect(requests.map((request) => request.headers?.Range)).toEqual([
                "lines=2-",
                "lines=3-4",
            ]);
        }).pipe(Effect.provide(layer));
    });
});