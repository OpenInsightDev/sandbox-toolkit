import { Context, Effect, Encoding, Layer, Ref } from "effect";
import type * as RefModule from "effect/Ref";
import { AuthenticationError, DigestChallengeError, TransportError, AbortError } from "#/error.ts";
import type { NormalizedWebDavConfig } from "#/config.ts";
import type { Headers, RequestData } from "#/domain.ts";
import { Transport, type WebDavHttpRequest, type WebDavHttpResponse } from "#/transport.ts";
import { getHeader } from "#/utils/headers.ts";

export interface AuthRequest extends WebDavHttpRequest {
    readonly authAttempt?: number;
}

export interface AuthChallengeResponse {
    readonly status: number;
    readonly headers: Headers;
}

export interface AuthRetry {
    readonly request: AuthRequest;
}

export interface AuthRejected {
    readonly reason: "unsupported" | "invalid" | "not-authenticated" | "not-replayable";
}

export interface Auth {
    readonly prepare: (request: AuthRequest) => Effect.Effect<AuthRequest, AuthenticationError>;
    readonly handleChallenge: (
        request: AuthRequest,
        response: AuthChallengeResponse,
    ) => Effect.Effect<AuthRetry | AuthRejected, AuthenticationError>;
}

export const Auth = Context.Service<Auth>("webdav/Auth");

export interface DigestChallenge {
    readonly scheme: "Digest";
    readonly realm: string;
    readonly nonce: string;
    readonly opaque?: string;
    readonly qop?: "auth" | "auth-int";
    readonly algorithm: "MD5" | "MD5-sess";
    readonly stale: boolean;
}

export interface DigestState {
    readonly username: string;
    readonly password?: string;
    readonly ha1?: string;
    readonly realm?: string;
    readonly nonce?: string;
    readonly opaque?: string;
    readonly qop?: "auth" | "auth-int";
    readonly algorithm: "MD5" | "MD5-sess";
    readonly cnonce?: string;
    readonly nonceCount: number;
    readonly hasDigestAuth: boolean;
}

const authError = (operation: string, message: string): AuthenticationError =>
    new AuthenticationError({ operation, message });

const digestError = (message: string): DigestChallengeError =>
    new DigestChallengeError({ operation: "digest-challenge", message });

const setHeader = (headers: Headers | undefined, name: string, value: string): Headers => {
    const result: Record<string, string> = {};
    for (const [key, headerValue] of Object.entries(headers ?? {})) {
        if (key.toLowerCase() !== name.toLowerCase()) result[key] = headerValue;
    }
    result[name] = value;
    return result;
};

const removeHeader = (headers: Headers | undefined, name: string): Headers => {
    const result: Record<string, string> = {};
    for (const [key, headerValue] of Object.entries(headers ?? {})) {
        if (key.toLowerCase() !== name.toLowerCase()) result[key] = headerValue;
    }
    return result;
};

const parseQuoted = (value: string): string => {
    const trimmed = value.trim();
    if (trimmed.startsWith('"') && trimmed.endsWith('"')) {
        return trimmed.slice(1, -1).replace(/\\([\\"])/g, "$1");
    }
    return trimmed;
};

const splitDirectives = (value: string): Array<string> => {
    const directives: Array<string> = [];
    let start = 0;
    let quote = false;
    let escaped = false;
    for (let index = 0; index < value.length; index += 1) {
        const char = value[index];
        if (escaped) {
            escaped = false;
        } else if (char === "\\" && quote) {
            escaped = true;
        } else if (char === '"') {
            quote = !quote;
        } else if (char === "," && !quote) {
            directives.push(value.slice(start, index));
            start = index + 1;
        }
    }
    directives.push(value.slice(start));
    return directives;
};

const parseDigestValue = (value: string): Record<string, string> => {
    const result: Record<string, string> = {};
    for (const directive of splitDirectives(value)) {
        const separator = directive.indexOf("=");
        if (separator < 1) continue;
        const key = directive.slice(0, separator).trim().toLowerCase();
        result[key] = parseQuoted(directive.slice(separator + 1));
    }
    return result;
};

/** Parses a WWW-Authenticate Digest challenge without exposing credential values. */
export const parseDigestChallenge = (
    header: string,
): Effect.Effect<DigestChallenge, DigestChallengeError> =>
    Effect.try({
        try: () => {
            const match = /^Digest\s+(.+)$/i.exec(header.trim());
            if (!match) throw digestError("WWW-Authenticate does not contain a Digest challenge");
            const values = parseDigestValue(match[1]);
            const realm = values.realm;
            const nonce = values.nonce;
            const algorithm = (values.algorithm ?? "MD5").toUpperCase();
            const qopValues = values.qop
                ?.split(",")
                .map((item) => item.trim().toLowerCase())
                .filter(
                    (item): item is "auth" | "auth-int" => item === "auth" || item === "auth-int",
                );
            if (!realm || !nonce) throw digestError("Digest challenge requires realm and nonce");
            if (algorithm !== "MD5" && algorithm !== "MD5-SESS") {
                throw digestError("Digest challenge uses an unsupported algorithm");
            }
            if (values.qop !== undefined && qopValues?.length === 0) {
                throw digestError("Digest challenge has no supported qop");
            }
            return {
                scheme: "Digest" as const,
                realm,
                nonce,
                ...(values.opaque === undefined ? {} : { opaque: values.opaque }),
                ...(qopValues?.[0] === undefined ? {} : { qop: qopValues[0] }),
                algorithm: algorithm === "MD5-SESS" ? ("MD5-sess" as const) : ("MD5" as const),
                stale: values.stale?.toLowerCase() === "true",
            };
        },
        catch: (error) =>
            error instanceof DigestChallengeError
                ? error
                : digestError("Digest challenge could not be parsed"),
    });

const rotate = (value: number, bits: number): number => (value << bits) | (value >>> (32 - bits));
const md5S: ReadonlyArray<number> = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9,
    14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15, 21,
    6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];
const md5K: ReadonlyArray<number> = Array.from({ length: 64 }, (_, index) =>
    Math.floor(Math.abs(Math.sin(index + 1)) * 0x100000000),
);

/** Small browser-compatible MD5 implementation used only for RFC 7616 compatibility. */
export const md5 = (input: string): string => {
    const bytes = new TextEncoder().encode(input);
    const bitLength = bytes.length * 8;
    const paddedLength = (((bytes.length + 8) >> 6) + 1) * 64;
    const message = new Uint8Array(paddedLength);
    message.set(bytes);
    message[bytes.length] = 0x80;
    const view = new DataView(message.buffer);
    view.setUint32(paddedLength - 8, bitLength >>> 0, true);
    view.setUint32(paddedLength - 4, Math.floor(bitLength / 0x100000000), true);

    let a0 = 0x67452301;
    let b0 = 0xefcdab89;
    let c0 = 0x98badcfe;
    let d0 = 0x10325476;
    for (let offset = 0; offset < message.length; offset += 64) {
        const words = Array.from({ length: 16 }, (_, index) =>
            view.getUint32(offset + index * 4, true),
        );
        let a = a0;
        let b = b0;
        let c = c0;
        let d = d0;
        for (let index = 0; index < 64; index += 1) {
            let f: number;
            let g: number;
            if (index < 16) {
                f = (b & c) | (~b & d);
                g = index;
            } else if (index < 32) {
                f = (d & b) | (~d & c);
                g = (5 * index + 1) % 16;
            } else if (index < 48) {
                f = b ^ c ^ d;
                g = (3 * index + 5) % 16;
            } else {
                f = c ^ (b | ~d);
                g = (7 * index) % 16;
            }
            const next = d;
            d = c;
            c = b;
            b = (b + rotate((a + f + md5K[index] + words[g]) | 0, md5S[index])) | 0;
            a = next;
        }
        a0 = (a0 + a) | 0;
        b0 = (b0 + b) | 0;
        c0 = (c0 + c) | 0;
        d0 = (d0 + d) | 0;
    }
    return [a0, b0, c0, d0]
        .flatMap((value) =>
            [0, 8, 16, 24].map((shift) => ((value >>> shift) & 0xff).toString(16).padStart(2, "0")),
        )
        .join("");
};

const isReplayable = (data: RequestData | undefined): boolean => {
    if (
        data === undefined ||
        typeof data === "string" ||
        data instanceof Uint8Array ||
        data instanceof ArrayBuffer
    ) {
        return true;
    }
    return !(
        typeof data === "object" &&
        data !== null &&
        "getReader" in data &&
        typeof data.getReader === "function"
    );
};

const bodyDigest = (data: RequestData | undefined): string => {
    if (data === undefined) return md5("");
    if (typeof data === "string") return md5(data);
    if (data instanceof Uint8Array) return md5(new TextDecoder().decode(data));
    if (data instanceof ArrayBuffer) return md5(new TextDecoder().decode(new Uint8Array(data)));
    return md5(JSON.stringify(data));
};

const basicHeader = (username: string, password: string): string =>
    `Basic ${Encoding.encodeBase64(`${username}:${password}`)}`;

const nonceValue = (count: number): string => count.toString(16).padStart(8, "0");

const digestHeader = (
    request: AuthRequest,
    state: DigestState,
    challenge: DigestChallenge,
    nonceCount: number,
): string => {
    const uri = new URL(request.url).pathname + new URL(request.url).search;
    const method = request.method.toUpperCase();
    const cnonce = state.cnonce ?? md5(`${challenge.nonce}:${state.username}`);
    const ha1Base =
        state.ha1 ?? md5(`${state.username}:${challenge.realm}:${state.password ?? ""}`);
    const ha1 =
        challenge.algorithm === "MD5-sess"
            ? md5(`${ha1Base}:${challenge.nonce}:${cnonce}`)
            : ha1Base;
    const ha2 =
        challenge.qop === "auth-int"
            ? md5(`${method}:${uri}:${bodyDigest(request.data)}`)
            : md5(`${method}:${uri}`);
    const nc = nonceValue(nonceCount);
    const response = challenge.qop
        ? md5(`${ha1}:${challenge.nonce}:${nc}:${cnonce}:${challenge.qop}:${ha2}`)
        : md5(`${ha1}:${challenge.nonce}:${ha2}`);
    const fields = [
        `username="${state.username}"`,
        `realm="${challenge.realm}"`,
        `nonce="${challenge.nonce}"`,
        `uri="${uri}"`,
        `response="${response}"`,
        `algorithm=${challenge.algorithm}`,
        ...(challenge.opaque === undefined ? [] : [`opaque="${challenge.opaque}"`]),
        ...(challenge.qop === undefined
            ? []
            : [`qop=${challenge.qop}`, `nc=${nc}`, `cnonce="${cnonce}"`]),
    ];
    return `Digest ${fields.join(", ")}`;
};

const createAuth = (
    config: NormalizedWebDavConfig,
    digestState: RefModule.Ref<DigestState>,
): Auth => {
    const prepare = (request: AuthRequest): Effect.Effect<AuthRequest, AuthenticationError> =>
        Effect.gen(function* () {
            if (getHeader(request.headers ?? {}, "Authorization") !== undefined) return request;
            if (config.authType === "none") return request;
            if (config.authType === "token") {
                const token = config.token;
                if (!token)
                    return yield* Effect.fail(
                        authError("auth.prepare", "token credentials are missing"),
                    );
                return {
                    ...request,
                    headers: setHeader(
                        request.headers,
                        "Authorization",
                        `${token.token_type} ${token.access_token}`,
                    ),
                };
            }
            if (config.authType === "password" || config.authType === "auto") {
                if (config.username === undefined || config.password === undefined) {
                    return yield* Effect.fail(
                        authError("auth.prepare", "password credentials are missing"),
                    );
                }
                const state = yield* Ref.get(digestState);
                if (
                    config.authType === "auto" &&
                    state.hasDigestAuth &&
                    state.nonce &&
                    state.realm
                ) {
                    const nonceCount = yield* Ref.modify(
                        digestState,
                        (current) =>
                            [
                                current.nonceCount + 1,
                                { ...current, nonceCount: current.nonceCount + 1 },
                            ] as const,
                    );
                    const challenge: DigestChallenge = {
                        scheme: "Digest",
                        realm: state.realm,
                        nonce: state.nonce,
                        ...(state.opaque === undefined ? {} : { opaque: state.opaque }),
                        ...(state.qop === undefined ? {} : { qop: state.qop }),
                        algorithm: state.algorithm,
                        stale: false,
                    };
                    return {
                        ...request,
                        headers: setHeader(
                            request.headers,
                            "Authorization",
                            digestHeader(request, state, challenge, nonceCount),
                        ),
                    };
                }
                return {
                    ...request,
                    headers: setHeader(
                        request.headers,
                        "Authorization",
                        basicHeader(config.username, config.password),
                    ),
                };
            }
            if (config.authType === "digest") {
                const state = yield* Ref.get(digestState);
                if (!state.hasDigestAuth || !state.nonce || !state.realm) return request;
                const nonceCount = yield* Ref.modify(
                    digestState,
                    (current) =>
                        [
                            current.nonceCount + 1,
                            { ...current, nonceCount: current.nonceCount + 1 },
                        ] as const,
                );
                const challenge: DigestChallenge = {
                    scheme: "Digest",
                    realm: state.realm,
                    nonce: state.nonce,
                    ...(state.opaque === undefined ? {} : { opaque: state.opaque }),
                    ...(state.qop === undefined ? {} : { qop: state.qop }),
                    algorithm: state.algorithm,
                    stale: false,
                };
                return {
                    ...request,
                    headers: setHeader(
                        request.headers,
                        "Authorization",
                        digestHeader(request, state, challenge, nonceCount),
                    ),
                };
            }
            return request;
        });

    const handleChallenge = (
        request: AuthRequest,
        response: AuthChallengeResponse,
    ): Effect.Effect<AuthRetry | AuthRejected, AuthenticationError> =>
        Effect.gen(function* () {
            if (response.status !== 401) return { reason: "not-authenticated" as const };
            if ((request.authAttempt ?? 0) >= 1) return { reason: "invalid" as const };
            const challengeHeader = getHeader(response.headers, "WWW-Authenticate");
            if (!challengeHeader) return { reason: "unsupported" as const };
            if (config.authType !== "auto" && config.authType !== "digest") {
                return { reason: "unsupported" as const };
            }
            const digestHeaderValue = challengeHeader
                .split(/,\s*(?=[A-Za-z]+\s)/)
                .find((item) => /^Digest\s/i.test(item));
            if (!digestHeaderValue) return { reason: "unsupported" as const };
            const challenge = yield* parseDigestChallenge(digestHeaderValue).pipe(
                Effect.mapError((error) => authError("auth.challenge", error.message)),
            );
            if (!isReplayable(request.data)) return { reason: "not-replayable" as const };
            const current = yield* Ref.get(digestState);
            const cnonce = current.cnonce ?? md5(`${challenge.nonce}:${config.username ?? ""}`);
            yield* Ref.set(digestState, {
                username: config.username ?? "",
                ...(config.password === undefined ? {} : { password: config.password }),
                ...(config.ha1 === undefined ? {} : { ha1: config.ha1 }),
                realm: challenge.realm,
                nonce: challenge.nonce,
                ...(challenge.opaque === undefined ? {} : { opaque: challenge.opaque }),
                ...(challenge.qop === undefined ? {} : { qop: challenge.qop }),
                algorithm: challenge.algorithm,
                cnonce,
                nonceCount: 0,
                hasDigestAuth: true,
            });
            return {
                request: {
                    ...request,
                    headers: removeHeader(request.headers, "Authorization"),
                    authAttempt: (request.authAttempt ?? 0) + 1,
                },
            };
        });

    return { prepare, handleChallenge };
};

export interface AuthLayerOptions {
    readonly config: NormalizedWebDavConfig;
}

export const AuthLive = (options: AuthLayerOptions): Layer.Layer<Auth> =>
    Layer.effect(
        Auth,
        Effect.gen(function* () {
            const digestState = yield* Ref.make<DigestState>({
                username: options.config.username ?? "",
                ...(options.config.password === undefined
                    ? {}
                    : { password: options.config.password }),
                ...(options.config.ha1 === undefined ? {} : { ha1: options.config.ha1 }),
                algorithm: "MD5",
                nonceCount: 0,
                hasDigestAuth: false,
            });
            return createAuth(options.config, digestState);
        }),
    );

export const AuthLayer = AuthLive;

export const authenticatedRequest = (
    request: WebDavHttpRequest,
): Effect.Effect<
    WebDavHttpResponse,
    AuthenticationError | TransportError | AbortError,
    Auth | Transport
> =>
    Effect.gen(function* () {
        const auth = yield* Auth;
        const transport = yield* Transport;
        const prepared = yield* auth.prepare(request);
        const response = yield* transport.execute(prepared);
        if (response.status !== 401) return response;
        const challenge = yield* auth.handleChallenge(prepared, response);
        if ("reason" in challenge) {
            return yield* Effect.fail(
                authError("auth.challenge", `authentication rejected: ${challenge.reason}`),
            );
        }
        return yield* transport.execute(yield* auth.prepare(challenge.request));
    });

export const AuthTest = (config: NormalizedWebDavConfig): Layer.Layer<Auth> => AuthLive({ config });