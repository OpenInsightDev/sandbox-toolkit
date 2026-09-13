import { Effect } from "effect";
import { describe, expect, it } from "@effect/vitest";
import { PartialUpdateFileContents } from "#/operations.ts";
import { responseFromText } from "#/transport.ts";
import { UnsupportedFeatureError } from "#/error.ts";
import { makeLayer } from "./helpers.ts";

describe("RFC 5789 PATCH support", () => {
    it.effect("uses PATCH method with Content-Range for partial updates", () => {
        const requests: Array<{
            method: string;
            headers?: Record<string, string>;
            data?: unknown;
        }> = [];
        const layer = makeLayer((request) => {
            requests.push(request);
            return Effect.succeed(responseFromText({ url: request.url, status: 204 }));
        });

        return Effect.gen(function* () {
            const partial = yield* PartialUpdateFileContents;

            // Partial update with range
            yield* partial.execute("/file.txt", "new content", {
                range: { start: 10, end: 20 },
            });

            expect(requests).toHaveLength(1);
            expect(requests[0]?.method).toBe("PATCH");
            expect(requests[0]?.headers?.["Content-Range"]).toBe("bytes 10-20/*");
            expect(requests[0]?.headers?.["Content-Type"]).toBe("application/octet-stream");
        }).pipe(Effect.provide(layer));
    });

    it.effect("uses custom content type when provided", () => {
        const requests: Array<{ headers?: Record<string, string> }> = [];
        const layer = makeLayer((request) => {
            requests.push(request);
            return Effect.succeed(responseFromText({ url: request.url, status: 200 }));
        });

        return Effect.gen(function* () {
            const partial = yield* PartialUpdateFileContents;

            yield* partial.execute("/file.json", '{"key":"value"}', {
                range: { start: 0, end: 14 },
                contentType: "application/json",
            });

            expect(requests[0]?.headers?.["Content-Type"]).toBe("application/json");
        }).pipe(Effect.provide(layer));
    });

    it.effect("calculates end position from data length when end not provided", () => {
        const requests: Array<{ headers?: Record<string, string> }> = [];
        const layer = makeLayer((request) => {
            requests.push(request);
            return Effect.succeed(responseFromText({ url: request.url, status: 204 }));
        });

        return Effect.gen(function* () {
            const partial = yield* PartialUpdateFileContents;

            // Range without end calculates end from data length
            const data = "append this"; // 11 bytes
            yield* partial.execute("/file.txt", data, {
                range: { start: 100 },
            });

            // start=100, length=11, so end=100+11-1=110
            expect(requests[0]?.headers?.["Content-Range"]).toBe("bytes 100-110/*");
        }).pipe(Effect.provide(layer));
    });

    it.effect("accepts 200, 204, and 206 status codes", () => {
        const testStatus = (status: number) => {
            const layer = makeLayer(() =>
                Effect.succeed(responseFromText({ url: "http://test.com/file", status })),
            );

            return Effect.gen(function* () {
                const partial = yield* PartialUpdateFileContents;
                const result = yield* partial.execute("/file.txt", "data", {
                    range: { start: 0, end: 3 },
                });
                expect(result).toBe(true);
            }).pipe(Effect.provide(layer));
        };

        return Effect.all([testStatus(200), testStatus(204), testStatus(206)]);
    });

    it.effect("throws UnsupportedFeatureError on 409 Conflict", () => {
        const layer = makeLayer(() =>
            Effect.succeed(responseFromText({ url: "http://test.com/file", status: 409 })),
        );

        return Effect.gen(function* () {
            const partial = yield* PartialUpdateFileContents;

            const result = yield* partial
                .execute("/file.txt", "data", {
                    range: { start: 0, end: 3 },
                })
                .pipe(Effect.flip);

            expect(result._tag).toBe("UnsupportedFeatureError");
            expect((result as UnsupportedFeatureError).message).toContain("does not support PATCH");
        }).pipe(Effect.provide(layer));
    });

    it.effect("falls back to PUT when no range is specified", () => {
        const requests: Array<{ method: string }> = [];
        const layer = makeLayer((request) => {
            requests.push(request);
            return Effect.succeed(responseFromText({ url: request.url, status: 201 }));
        });

        return Effect.gen(function* () {
            const partial = yield* PartialUpdateFileContents;

            // No range means full file replacement
            yield* partial.execute("/file.txt", "complete new content");

            expect(requests).toHaveLength(1);
            expect(requests[0]?.method).toBe("PUT");
        }).pipe(Effect.provide(layer));
    });

    it.effect("works with binary data", () => {
        const requests: Array<{ data?: unknown }> = [];
        const layer = makeLayer((request) => {
            requests.push(request);
            return Effect.succeed(responseFromText({ url: request.url, status: 204 }));
        });

        return Effect.gen(function* () {
            const partial = yield* PartialUpdateFileContents;
            const binaryData = new Uint8Array([0x48, 0x65, 0x6c, 0x6c, 0x6f]); // "Hello"

            yield* partial.execute("/file.bin", binaryData, {
                range: { start: 50, end: 54 },
            });

            expect(requests[0]?.data).toEqual(binaryData);
        }).pipe(Effect.provide(layer));
    });

    it.effect("preserves custom headers", () => {
        const requests: Array<{ headers?: Record<string, string> }> = [];
        const layer = makeLayer((request) => {
            requests.push(request);
            return Effect.succeed(responseFromText({ url: request.url, status: 200 }));
        });

        return Effect.gen(function* () {
            const partial = yield* PartialUpdateFileContents;

            yield* partial.execute("/file.txt", "data", {
                range: { start: 0, end: 3 },
                headers: {
                    "X-Custom-Header": "custom-value",
                    Authorization: "Bearer token123",
                },
            });

            expect(requests[0]?.headers?.["X-Custom-Header"]).toBe("custom-value");
            expect(requests[0]?.headers?.["Authorization"]).toBe("Bearer token123");
            expect(requests[0]?.headers?.["Content-Range"]).toBe("bytes 0-3/*");
        }).pipe(Effect.provide(layer));
    });

    it.effect("handles ArrayBuffer data", () => {
        const requests: Array<{ method: string }> = [];
        const layer = makeLayer((request) => {
            requests.push(request);
            return Effect.succeed(responseFromText({ url: request.url, status: 204 }));
        });

        return Effect.gen(function* () {
            const partial = yield* PartialUpdateFileContents;
            const buffer = new ArrayBuffer(8);
            const view = new Uint8Array(buffer);
            view.set([1, 2, 3, 4, 5, 6, 7, 8]);

            yield* partial.execute("/file.bin", buffer, {
                range: { start: 0, end: 7 },
            });

            expect(requests[0]?.method).toBe("PATCH");
        }).pipe(Effect.provide(layer));
    });
});