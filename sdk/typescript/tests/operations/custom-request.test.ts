import { Effect } from "effect";
import { describe, expect, it } from "@effect/vitest";
import { CustomRequest } from "#/operations.ts";
import { responseFromText } from "#/transport.ts";
import { makeLayer } from "./helpers.ts";

describe("WebDAV custom request", () => {
    it.effect("preserves the custom method, body, headers, and detailed response", () => {
        let received:
            | { method: string; data?: unknown; headers?: Record<string, string> }
            | undefined;
        const layer = makeLayer((request) => {
            received = request;
            return Effect.succeed(
                responseFromText({
                    url: request.url,
                    status: 207,
                    statusText: "Multi-Status",
                    headers: { "Content-Type": "text/plain" },
                    body: "custom response",
                }),
            );
        });
        return Effect.gen(function* () {
            const custom = yield* CustomRequest;
            const response = yield* custom.execute({
                url: "https://dav.example.test/report",
                method: "REPORT",
                headers: { Depth: "1" },
                data: "<report />",
            });
            expect(received).toMatchObject({
                method: "REPORT",
                data: "<report />",
                headers: { Depth: "1", "X-Client": "test" },
            });
            expect(response).toMatchObject({
                status: 207,
                statusText: "Multi-Status",
                body: "custom response",
            });
        }).pipe(Effect.provide(layer));
    });
});