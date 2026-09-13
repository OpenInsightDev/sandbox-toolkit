import { NodeHttpServer } from "@effect/platform-node";
import { Cause, Effect, Exit, Fiber, Layer, Stream } from "effect";
import { describe, expect, it } from "@effect/vitest";
import { HttpRouter, HttpServer, HttpServerResponse } from "effect/unstable/http";
import * as NetAddress from "effect/unstable/net/NetAddress";
import {
    Transport,
    FetchHttpTransport,
    TestTransport,
    responseFromBytes,
    responseFromText,
} from "#/transport.ts";
import { AbortError, TransportError } from "#/error.ts";

const request = {
    url: "https://dav.example.test/files/notes.txt",
    method: "PROPFIND",
    headers: { Depth: "1", "X-Test": "transport" },
    data: "request-body",
};

const collectText = (stream: Stream.Stream<Uint8Array, unknown>) =>
    Stream.runCollect(stream).pipe(
        Effect.map((chunks) =>
            new TextDecoder().decode(Uint8Array.from(chunks.flatMap((chunk) => [...chunk]))),
        ),
    );

const failureError = <E>(exit: Exit.Exit<unknown, E>): E | undefined => {
    if (!Exit.isFailure(exit)) return undefined;
    const error = Cause.findErrorOption(exit.cause);
    return error._tag === "Some" ? error.value : undefined;
};

describe("Effect WebDAV HTTP transport", () => {
    it.effect("TestTransport preserves request, metadata, and response stream data", () => {
        let received: typeof request | undefined;
        const layer = TestTransport({
            execute: (input) => {
                received = input as typeof request;
                return Effect.succeed(
                    responseFromText({
                        url: input.url,
                        status: 207,
                        statusText: "Multi-Status",
                        headers: { "Content-Type": "text/plain" },
                        body: "ok",
                    }),
                );
            },
        });

        return Effect.gen(function* () {
            const transport = yield* Transport;
            const response = yield* transport.execute(request);
            const body = yield* collectText(response.stream);

            expect(received).toEqual(request);
            expect(response.status).toBe(207);
            expect(response.statusText).toBe("Multi-Status");
            expect(response.headers["Content-Type"]).toBe("text/plain");
            expect(body).toBe("ok");
        }).pipe(Effect.provide(layer));
    });

    it.effect("adapts DAV methods, text body, headers, status, and credentials", () => {
        const originalFetch = globalThis.fetch;
        let fetchRequest: Request | undefined;
        globalThis.fetch = async (input, init) => {
            fetchRequest = new Request(input, init);
            return new Response("response-body", {
                status: 207,
                statusText: "Multi-Status",
                headers: { "Content-Type": "text/plain" },
            });
        };

        return Effect.gen(function* () {
            const transport = yield* Transport;
            const response = yield* transport.execute({ ...request, withCredentials: true });
            const body = yield* collectText(response.stream);

            expect(fetchRequest?.method).toBe("PROPFIND");
            expect(fetchRequest?.headers.get("depth")).toBe("1");
            expect(fetchRequest?.headers.get("x-test")).toBe("transport");
            expect(yield* Effect.promise(() => fetchRequest?.text() ?? Promise.resolve(""))).toBe(
                "request-body",
            );
            expect(fetchRequest?.credentials).toBe("include");
            expect(response.status).toBe(207);
            expect(response.statusText).toBe("Multi-Status");
            expect(response.url).toBe(request.url);
            expect(body).toBe("response-body");
        }).pipe(
            Effect.provide(FetchHttpTransport.browserLayer),
            Effect.ensuring(
                Effect.sync(() => {
                    globalThis.fetch = originalFetch;
                }),
            ),
        );
    });

    it.effect("encodes Uint8Array, ArrayBuffer, and object request bodies", () => {
        const originalFetch = globalThis.fetch;
        const bodies: Array<string> = [];
        globalThis.fetch = async (input, init) => {
            bodies.push(await new Request(input, init).text());
            return new Response("ok", { status: 200 });
        };

        return Effect.gen(function* () {
            const transport = yield* Transport;
            yield* transport.execute({
                url: request.url,
                method: "PUT",
                data: new Uint8Array([1, 2, 3]),
            });
            yield* transport.execute({
                url: request.url,
                method: "PUT",
                data: new Uint8Array([65, 66]).buffer,
            });
            yield* transport.execute({ url: request.url, method: "PUT", data: { enabled: true } });

            expect(bodies).toEqual(["\u0001\u0002\u0003", "AB", '{"enabled":true}']);
        }).pipe(
            Effect.provide(FetchHttpTransport.nodeLayer),
            Effect.ensuring(
                Effect.sync(() => {
                    globalThis.fetch = originalFetch;
                }),
            ),
        );
    });

    it.effect("fails an already aborted request without calling fetch", () => {
        const controller = new AbortController();
        controller.abort();
        const originalFetch = globalThis.fetch;
        let called = false;
        globalThis.fetch = async () => {
            called = true;
            return new Response();
        };

        return Effect.gen(function* () {
            const transport = yield* Transport;
            const exit = yield* Effect.exit(
                transport.execute({ ...request, signal: controller.signal }),
            );
            const error = failureError(exit);

            expect(error).toBeInstanceOf(AbortError);
            expect(called).toBe(false);
        }).pipe(
            Effect.provide(FetchHttpTransport.nodeLayer),
            Effect.ensuring(
                Effect.sync(() => {
                    globalThis.fetch = originalFetch;
                }),
            ),
        );
    });

    it.effect("maps ordinary fetch failures to TransportError", () => {
        const originalFetch = globalThis.fetch;
        globalThis.fetch = async () => {
            throw new Error("connection refused");
        };

        return Effect.gen(function* () {
            const transport = yield* Transport;
            const exit = yield* Effect.exit(transport.execute(request));
            const error = failureError(exit);

            expect(error).toBeInstanceOf(TransportError);
            expect(error?._tag).toBe("TransportError");
            expect(error?.cause).toContain("Transport error");
            expect(error?.url).toBe(request.url);
        }).pipe(
            Effect.provide(FetchHttpTransport.nodeLayer),
            Effect.ensuring(
                Effect.sync(() => {
                    globalThis.fetch = originalFetch;
                }),
            ),
        );
    });

    it.effect("propagates external cancellation to an in-flight fetch", () => {
        const originalFetch = globalThis.fetch;
        const controller = new AbortController();
        let fetchSignal: AbortSignal | undefined;
        let fetchStarted: (() => void) | undefined;
        const started = new Promise<void>((resolve) => {
            fetchStarted = resolve;
        });
        globalThis.fetch = (_input, init) => {
            fetchSignal = init?.signal ?? undefined;
            return new Promise<Response>((_resolve, reject) => {
                init?.signal?.addEventListener(
                    "abort",
                    () => reject(new DOMException("Aborted", "AbortError")),
                    { once: true },
                );
                fetchStarted?.();
            });
        };

        return Effect.gen(function* () {
            const transport = yield* Transport;
            const fiber = yield* Effect.forkChild(
                transport.execute({ ...request, signal: controller.signal }),
            );
            yield* Effect.promise(() => started);
            controller.abort();
            const exit = yield* Fiber.join(fiber).pipe(Effect.exit);

            expect(fetchSignal?.aborted).toBe(true);
            expect(failureError(exit)).toBeInstanceOf(AbortError);
        }).pipe(
            Effect.provide(FetchHttpTransport.nodeLayer),
            Effect.ensuring(
                Effect.sync(() => {
                    globalThis.fetch = originalFetch;
                }),
            ),
        );
    });

    it.effect("aborts the underlying response when the response stream is consumed", () => {
        const originalFetch = globalThis.fetch;
        let responseSignal: AbortSignal | undefined;
        globalThis.fetch = async (_input, init) => {
            responseSignal = init?.signal ?? undefined;
            return new Response(
                new ReadableStream<Uint8Array>({
                    start(controller) {
                        controller.enqueue(new TextEncoder().encode("chunk"));
                        controller.close();
                    },
                }),
                { status: 200, statusText: "OK" },
            );
        };

        return Effect.gen(function* () {
            const transport = yield* Transport;
            const response = yield* transport.execute(request);
            expect(yield* collectText(response.stream)).toBe("chunk");
            expect(responseSignal?.aborted).toBe(true);
        }).pipe(
            Effect.provide(FetchHttpTransport.nodeLayer),
            Effect.ensuring(
                Effect.sync(() => {
                    globalThis.fetch = originalFetch;
                }),
            ),
        );
    });

    it.effect("releases the response when stream consumption fails", () => {
        const originalFetch = globalThis.fetch;
        let responseSignal: AbortSignal | undefined;
        globalThis.fetch = async (_input, init) => {
            responseSignal = init?.signal ?? undefined;
            return new Response(
                new ReadableStream<Uint8Array>({
                    start(controller) {
                        controller.error(new Error("response stream failed"));
                    },
                }),
                { status: 200, statusText: "OK" },
            );
        };

        return Effect.gen(function* () {
            const transport = yield* Transport;
            const response = yield* transport.execute(request);
            const exit = yield* Effect.exit(collectText(response.stream));

            expect(Exit.isFailure(exit)).toBe(true);
            expect(responseSignal?.aborted).toBe(true);
        }).pipe(
            Effect.provide(FetchHttpTransport.nodeLayer),
            Effect.ensuring(
                Effect.sync(() => {
                    globalThis.fetch = originalFetch;
                }),
            ),
        );
    });

    it.effect("releases the response when stream consumption stops early", () => {
        const originalFetch = globalThis.fetch;
        let responseSignal: AbortSignal | undefined;
        let streamCancelled: (() => void) | undefined;
        const cancelled = new Promise<void>((resolve) => {
            streamCancelled = resolve;
        });
        globalThis.fetch = async (_input, init) => {
            responseSignal = init?.signal ?? undefined;
            return new Response(
                new ReadableStream<Uint8Array>({
                    start(controller) {
                        controller.enqueue(new TextEncoder().encode("first"));
                    },
                    pull(controller) {
                        controller.enqueue(new TextEncoder().encode("second"));
                    },
                    cancel() {
                        streamCancelled?.();
                    },
                }),
                { status: 200, statusText: "OK" },
            );
        };

        return Effect.gen(function* () {
            const transport = yield* Transport;
            const response = yield* transport.execute(request);
            const firstChunk = yield* Stream.runCollect(response.stream.pipe(Stream.take(1)));
            yield* Effect.promise(() => cancelled);

            expect(new TextDecoder().decode(firstChunk[0])).toBe("first");
            expect(responseSignal?.aborted).toBe(true);
        }).pipe(
            Effect.provide(FetchHttpTransport.nodeLayer),
            Effect.ensuring(
                Effect.sync(() => {
                    globalThis.fetch = originalFetch;
                }),
            ),
        );
    });

    it.effect("consumes a response body only once", () => {
        const originalFetch = globalThis.fetch;
        globalThis.fetch = async () => new Response("once", { status: 200 });

        return Effect.gen(function* () {
            const transport = yield* Transport;
            const response = yield* transport.execute(request);
            expect(yield* collectText(response.stream)).toBe("once");
            const secondRead = yield* Effect.exit(collectText(response.stream));

            expect(Exit.isFailure(secondRead)).toBe(true);
        }).pipe(
            Effect.provide(FetchHttpTransport.nodeLayer),
            Effect.ensuring(
                Effect.sync(() => {
                    globalThis.fetch = originalFetch;
                }),
            ),
        );
    });

    it.effect("executes GET and PUT against a Node HTTP fixture", () => {
        const received: Array<{ method: string; body: string }> = [];
        const routes = Layer.mergeAll(
            HttpRouter.add(
                "GET",
                "/fixture",
                Effect.sync(() => {
                    received.push({ method: "GET", body: "" });
                    return HttpServerResponse.text("node-get", {
                        headers: { "content-type": "text/plain" },
                    });
                }),
            ),
            HttpRouter.add("PUT", "/fixture", (request) =>
                request.text.pipe(
                    Effect.map((body) => {
                        received.push({ method: "PUT", body });
                        return HttpServerResponse.text(`node-put:${body}`, {
                            headers: { "content-type": "text/plain" },
                        });
                    }),
                ),
            ),
        );
        const serverLayer = HttpRouter.serve(routes).pipe(
            Layer.provideMerge(NodeHttpServer.layerTest),
        );

        return Effect.gen(function* () {
            const server = yield* HttpServer.HttpServer;
            if (server.address._tag === "UnixPathAddress") {
                return yield* Effect.die("Node HTTP fixture did not expose a TCP address");
            }
            const baseUrl = NetAddress.formatUrlUnsafe(server.address, "http");
            const transport = yield* Transport;
            const getResponse = yield* transport.execute({
                url: `${baseUrl}/fixture`,
                method: "GET",
            });
            const putResponse = yield* transport.execute({
                url: `${baseUrl}/fixture`,
                method: "PUT",
                headers: { "Content-Type": "text/plain" },
                data: "node-body",
            });

            expect(yield* collectText(getResponse.stream)).toBe("node-get");
            expect(yield* collectText(putResponse.stream)).toBe("node-put:node-body");
            expect(received).toEqual([
                { method: "GET", body: "" },
                { method: "PUT", body: "node-body" },
            ]);
        }).pipe(Effect.provide(FetchHttpTransport.nodeLayer), Effect.provide(serverLayer));
    });

    it.effect("exposes equivalent node and browser layers", () =>
        Effect.sync(() => {
            expect(Layer.isLayer(FetchHttpTransport.nodeLayer)).toBe(true);
            expect(Layer.isLayer(FetchHttpTransport.browserLayer)).toBe(true);
            expect(Layer.isLayer(FetchHttpTransport.layer)).toBe(true);
            expect(responseFromBytes({ url: request.url, body: new Uint8Array([1]) }).status).toBe(
                200,
            );
        }),
    );
});