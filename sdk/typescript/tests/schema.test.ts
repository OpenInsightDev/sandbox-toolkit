import { Cause, Effect, Exit, Schema } from "effect";
import { describe, expect, it } from "@effect/vitest";
import {
    AuthType,
    GetDirectoryContentsOptions,
    GetFileContentsOptions,
    decodeWebDavConfig,
    decodeWebDavConfigSync,
} from "#/config.ts";
import {
    DAVProperties,
    DAVResult,
    DiskQuota,
    EntityDecoderOptions,
    FileStat,
    HttpMethod,
    Path,
    Range,
    RequestData,
    ResponseDataDetailed,
    Url,
} from "#/domain.ts";
import {
    AbortError,
    AuthenticationError,
    ConfigError,
    InvalidConfigError,
    InvalidDavResponseError,
    InvalidPathError,
    InvalidRangeError,
    ResponseDecodeError,
    StreamError,
    TransportError,
    WebDavError,
    XmlParseError,
} from "#/error.ts";

describe("reference-compatible schema contracts", () => {
    it.effect("normalizes the minimal client configuration", () =>
        Effect.sync(() => {
            const config = decodeWebDavConfigSync({ remoteUrl: "https://dav.example.test/root" });

            expect(config).toMatchObject({
                remoteUrl: "https://dav.example.test/root",
                authType: "none",
                headers: {},
                withCredentials: false,
            });
            expect(config.contactHref).toContain("LOCK_CONTACT.md");
        }),
    );

    it.effect("accepts every authentication mode and preserves optional values", () =>
        Effect.sync(() => {
            expect(Schema.decodeUnknownSync(AuthType)("auto")).toBe("auto");
            expect(Schema.decodeUnknownSync(AuthType)("digest")).toBe("digest");
            expect(Schema.decodeUnknownSync(AuthType)("none")).toBe("none");
            expect(Schema.decodeUnknownSync(AuthType)("password")).toBe("password");
            expect(Schema.decodeUnknownSync(AuthType)("token")).toBe("token");

            const config = decodeWebDavConfigSync({
                remoteUrl: "https://dav.example.test/root",
                remoteBasePath: "/dav",
                authType: "token",
                token: { access_token: "secret", token_type: "Bearer", refresh_token: "refresh" },
                headers: { "X-Test": "1" },
                withCredentials: true,
                entityDecoder: { limit: { maxTotalExpansions: 10, maxExpandedLength: 1000 } },
            });

            expect(config).toMatchObject({
                remoteBasePath: "/dav",
                authType: "token",
                token: { access_token: "secret", token_type: "Bearer", refresh_token: "refresh" },
                headers: { "X-Test": "1" },
                withCredentials: true,
                entityDecoder: { limit: { maxTotalExpansions: 10, maxExpandedLength: 1000 } },
            });
        }),
    );

    it.effect("rejects invalid URL, path, method, range, and decoder values", () =>
        Effect.sync(() => {
            expect(() => decodeWebDavConfigSync({ remoteUrl: "file:///tmp/dav" })).toThrowError(
                InvalidConfigError,
            );
            expect(() => Schema.decodeUnknownSync(Url)("/relative/path")).toThrow();
            expect(() => Schema.decodeUnknownSync(Path)("/invalid\u0000path")).toThrow();
            expect(() => Schema.decodeUnknownSync(HttpMethod)("")).toThrow();
            expect(() => Schema.decodeUnknownSync(Range)({ start: -1 })).toThrow();
            expect(() => Schema.decodeUnknownSync(Range)({ start: 10, end: 2 })).toThrow();
            expect(() =>
                Schema.decodeUnknownSync(EntityDecoderOptions)({
                    limit: { maxTotalExpansions: 1.5 },
                }),
            ).toThrow();
        }),
    );

    it.effect("reports all authentication configuration failures as InvalidConfigError", () =>
        Effect.gen(function* () {
            const cases = [
                { authType: "password", username: "alice" },
                { authType: "digest", username: "alice" },
                { authType: "token" },
                { authType: "none", username: "alice" },
                { authType: "auto" },
            ] as const;

            for (const options of cases) {
                const exit = yield* Effect.exit(
                    decodeWebDavConfig({ remoteUrl: "https://dav.example.test", ...options }),
                );
                expect(Exit.isFailure(exit)).toBe(true);
                if (Exit.isFailure(exit)) {
                    const error = Cause.findErrorOption(exit.cause);
                    expect(error._tag).toBe("Some");
                    if (error._tag === "Some") {
                        expect(error.value).toBeInstanceOf(InvalidConfigError);
                        expect(error.value._tag).toBe("InvalidConfigError");
                    }
                }
            }
        }),
    );

    it.effect("preserves dynamic DAV properties and response data shapes", () =>
        Effect.sync(() => {
            const stat = Schema.decodeUnknownSync(FileStat)({
                filename: "/notes.txt",
                basename: "notes.txt",
                lastmod: "Mon, 01 Jan 2024 00:00:00 GMT",
                size: 10,
                type: "file",
                etag: null,
                props: { displayname: "notes.txt", "x-custom": { raw: true } },
            });
            expect(stat.props?.["x-custom"]).toEqual({ raw: true });

            expect(
                Schema.decodeUnknownSync(DAVProperties)({
                    "x:metadata": { values: ["one", "two"] },
                }),
            ).toEqual({ "x:metadata": { values: ["one", "two"] } });

            expect(
                Schema.decodeUnknownSync(DAVResult)({
                    multistatus: { response: [{ href: "/notes.txt", status: "HTTP/1.1 200 OK" }] },
                }),
            ).toEqual({
                multistatus: { response: [{ href: "/notes.txt", status: "HTTP/1.1 200 OK" }] },
            });

            expect(
                Schema.decodeUnknownSync(DiskQuota)({ used: 10, available: "unlimited" }),
            ).toEqual({
                used: 10,
                available: "unlimited",
            });
            expect(Schema.decodeUnknownSync(RequestData)({ content: true })).toEqual({
                content: true,
            });
            expect(
                Schema.decodeUnknownSync(GetDirectoryContentsOptions)({
                    deep: true,
                    glob: "/**/*.txt",
                }),
            ).toEqual({ deep: true, glob: "/**/*.txt" });
            expect(Schema.decodeUnknownSync(GetFileContentsOptions)({ format: "text" })).toEqual({
                format: "text",
            });
            expect(
                Schema.decodeUnknownSync(ResponseDataDetailed(Schema.String))({
                    data: "ok",
                    headers: { "Content-Type": "text/plain" },
                    status: 207,
                    statusText: "Multi-Status",
                    url: "https://dav.example.test/notes.txt",
                }),
            ).toMatchObject({ data: "ok", status: 207 });
        }),
    );

    it.effect("exposes a tagged WebDAV error union", () =>
        Effect.sync(() => {
            const errors = [
                new ConfigError({ operation: "config", message: "config" }),
                new InvalidConfigError({ operation: "config", message: "invalid config" }),
                new InvalidPathError({ operation: "path", message: "invalid path" }),
                new InvalidRangeError({ operation: "range", message: "invalid range" }),
                new TransportError({ operation: "request", message: "transport" }),
                new AuthenticationError({ operation: "auth", message: "auth" }),
                new XmlParseError({ operation: "parse", message: "xml" }),
                new InvalidDavResponseError({ operation: "parse", message: "dav" }),
                new ResponseDecodeError({ operation: "decode", message: "decode" }),
                new StreamError({ operation: "stream", message: "stream" }),
                new AbortError({ operation: "request", message: "abort" }),
            ];

            for (const error of errors) {
                expect(Schema.is(WebDavError)(error)).toBe(true);
            }
        }),
    );
});