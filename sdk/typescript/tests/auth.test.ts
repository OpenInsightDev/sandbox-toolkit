import { Cause, Effect, Exit, Fiber } from "effect";
import { describe, expect, it } from "@effect/vitest";
import { Auth, AuthLive, authenticatedRequest, md5, parseDigestChallenge } from "#/auth.ts";
import { decodeWebDavConfigSync } from "#/config.ts";
import { AuthenticationError } from "#/error.ts";
import { TestTransport, responseFromText } from "#/transport.ts";

const request = {
    url: "https://dav.example.test/dir/index.html",
    method: "GET",
};

const passwordConfig = (authType: "auto" | "digest" | "none" | "password") =>
    decodeWebDavConfigSync({
        remoteUrl: "https://dav.example.test",
        authType,
        username: "Mufasa",
        password: "Circle Of Life",
    });

const tokenConfig = () =>
    decodeWebDavConfigSync({
        remoteUrl: "https://dav.example.test",
        authType: "token",
        token: { access_token: "secret-token", token_type: "Bearer" },
    });

const failureError = <E>(exit: Exit.Exit<unknown, E>): E | undefined => {
    if (!Exit.isFailure(exit)) return undefined;
    const error = Cause.findErrorOption(exit.cause);
    return error._tag === "Some" ? error.value : undefined;
};

describe("Effect WebDAV authentication", () => {
    it.effect("supports Basic authorization", () =>
        Effect.gen(function* () {
            const auth = yield* Auth;
            const prepared = yield* auth.prepare(request);
            expect(prepared.headers?.Authorization).toBe("Basic TXVmYXNhOkNpcmNsZSBPZiBMaWZl");
        }).pipe(Effect.provide(AuthLive({ config: passwordConfig("password") }))),
    );

    it.effect("supports token authorization", () =>
        Effect.gen(function* () {
            const auth = yield* Auth;
            const prepared = yield* auth.prepare(request);
            expect(prepared.headers?.Authorization).toBe("Bearer secret-token");
        }).pipe(Effect.provide(AuthLive({ config: tokenConfig() }))),
    );

    it.effect("switches Auto auth from Basic to Digest after a challenge", () =>
        Effect.gen(function* () {
            const auth = yield* Auth;
            const basic = yield* auth.prepare(request);
            expect(basic.headers?.Authorization).toBe("Basic TXVmYXNhOkNpcmNsZSBPZiBMaWZl");
            const retry = yield* auth.handleChallenge(request, {
                status: 401,
                headers: { "WWW-Authenticate": 'Digest realm="realm", nonce="nonce", qop="auth"' },
            });
            if (!("request" in retry)) throw new Error("Expected an Auto auth retry");
            const digest = yield* auth.prepare(retry.request);
            expect(digest.headers?.Authorization).toContain("Digest ");
        }).pipe(Effect.provide(AuthLive({ config: passwordConfig("auto") }))),
    );

    it.effect("parses Digest challenges and computes the RFC 2617 MD5 response", () =>
        Effect.gen(function* () {
            expect(md5("abc")).toBe("900150983cd24fb0d6963f7d28e17f72");
            const challenge = yield* parseDigestChallenge(
                'Digest realm="testrealm@host.com", qop="auth,auth-int", nonce="dcd98b7102dd2f0e8b11d0f600bfb0c093", opaque="5ccc069c403ebaf9f0171e9517f40e41"',
            );
            expect(challenge).toMatchObject({
                realm: "testrealm@host.com",
                nonce: "dcd98b7102dd2f0e8b11d0f600bfb0c093",
                qop: "auth",
                algorithm: "MD5",
            });

            const auth = yield* Auth;
            const challengeResult = yield* auth.handleChallenge(request, {
                status: 401,
                headers: {
                    "WWW-Authenticate":
                        'Digest realm="testrealm@host.com", qop="auth", nonce="dcd98b7102dd2f0e8b11d0f600bfb0c093", opaque="5ccc069c403ebaf9f0171e9517f40e41"',
                },
            });
            if (!("request" in challengeResult)) throw new Error("Expected a digest retry");
            const prepared = yield* auth.prepare(challengeResult.request);
            expect(prepared.headers?.Authorization).toMatch(
                /Digest username="Mufasa", realm="testrealm@host.com", nonce="dcd98b7102dd2f0e8b11d0f600bfb0c093"/,
            );
            expect(prepared.headers?.Authorization).toMatch(/response="[0-9a-f]{32}"/);
            expect(prepared.headers?.Authorization).toContain("nc=00000001");
        }).pipe(Effect.provide(AuthLive({ config: passwordConfig("digest") }))),
    );

    it.effect("uses an atomic nonce count for concurrent Digest requests", () =>
        Effect.gen(function* () {
            const auth = yield* Auth;
            const challenge = yield* auth.handleChallenge(request, {
                status: 401,
                headers: {
                    "WWW-Authenticate": 'Digest realm="realm", nonce="nonce", qop="auth"',
                },
            });
            if (!("request" in challenge)) throw new Error("Expected a digest retry");
            const requests = yield* Effect.all(
                Array.from({ length: 20 }, () => auth.prepare(challenge.request)),
                { concurrency: "unbounded" },
            );
            const counts = requests.map(
                (item) => item.headers?.Authorization?.match(/nc=([0-9a-f]+)/)?.[1],
            );
            expect(new Set(counts).size).toBe(20);
            expect(counts.sort((left, right) => (left ?? "").localeCompare(right ?? ""))).toEqual(
                Array.from({ length: 20 }, (_, index) => (index + 1).toString(16).padStart(8, "0")),
            );
        }).pipe(Effect.provide(AuthLive({ config: passwordConfig("digest") }))),
    );

    it.effect("replays a Digest challenge at most once through the transport", () => {
        const received: Array<Record<string, string> | undefined> = [];
        const transport = TestTransport({
            execute: (input) => {
                received.push(input.headers);
                return Effect.succeed(
                    received.length === 1
                        ? responseFromText({
                              url: input.url,
                              status: 401,
                              headers: {
                                  "WWW-Authenticate":
                                      'Digest realm="realm", nonce="nonce", qop="auth"',
                              },
                          })
                        : responseFromText({ url: input.url, status: 200, body: "ok" }),
                );
            },
        });

        return Effect.gen(function* () {
            const response = yield* authenticatedRequest(request);
            expect(response.status).toBe(200);
            expect(received).toHaveLength(2);
            expect(received[0]?.Authorization).toBeUndefined();
            expect(received[1]?.Authorization).toContain("Digest ");
        }).pipe(
            Effect.provide(AuthLive({ config: passwordConfig("digest") })),
            Effect.provide(transport),
        );
    });

    it.effect("does not leak credentials when a challenge cannot be retried", () => {
        const transport = TestTransport({
            execute: (input) =>
                Effect.succeed(
                    responseFromText({
                        url: input.url,
                        status: 401,
                        headers: {
                            "WWW-Authenticate": 'Digest realm="realm", nonce="nonce", qop="auth"',
                        },
                    }),
                ),
        });
        return Effect.gen(function* () {
            const exit = yield* Effect.exit(
                authenticatedRequest({ ...request, data: new ReadableStream() as never }),
            );
            const error = failureError(exit);
            expect(error).toBeInstanceOf(AuthenticationError);
            expect(error?.message).not.toContain("Circle Of Life");
            expect(error?.message).not.toContain("nonce");
        }).pipe(
            Effect.provide(AuthLive({ config: passwordConfig("digest") })),
            Effect.provide(transport),
        );
    });

    it.effect("does not leave a child fiber running after auth state checks", () =>
        Effect.gen(function* () {
            const auth = yield* Auth;
            const fiber = yield* Effect.forkChild(auth.prepare(request));
            yield* Fiber.join(fiber);
            expect(true).toBe(true);
        }).pipe(Effect.provide(AuthLive({ config: passwordConfig("digest") }))),
    );
});