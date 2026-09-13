import { Effect, Exit } from "effect";
import { describe, expect, it } from "@effect/vitest";
import type { DAVResult } from "#/domain.ts";
import { DavXmlCodecLive, collectDavPrefixes } from "#/xml.ts";
import { readResponse } from "../fixtures/read.ts";

const parseFixture = (name: string) => DavXmlCodecLive.parseMultiStatus(readResponse(name));

const firstProp = (result: DAVResult): Record<string, unknown> =>
    (result.multistatus.response[0]?.propstat?.prop ?? {}) as Record<string, unknown>;

const nextProp = (result: DAVResult, index: number): Record<string, unknown> =>
    (result.multistatus.response[index]?.propstat?.prop ?? {}) as Record<string, unknown>;

describe("migrated DAV XML fixtures", () => {
    it.effect("keeps numeric-looking display names as strings", () =>
        Effect.gen(function* () {
            const numeric = yield* parseFixture("propfind-numeric-displayname");
            expect(firstProp(numeric).displayname).toBe("1");

            const floating = yield* parseFixture("propfind-float-like-displayname");
            // The trailing zero must survive: "2024.10" is not parsed into 2024.1.
            expect(firstProp(floating).displayname).toBe("2024.10");
        }),
    );

    it.effect("normalizes DAV properties regardless of the serialized prefix", () =>
        Effect.gen(function* () {
            const lower = yield* parseFixture("propfind-numeric-displayname");
            expect(firstProp(lower)).toMatchObject({
                getlastmodified: "Wed, 24 Jul 2024 19:46:09 GMT",
                getetag: '"66a15a0171527"',
                "quota-available-bytes": "-3",
                creationdate: "1970-01-01T00:00:00+00:00",
                resourcetype: { collection: "" },
            });

            // The seafile server serialises DAV: with the arbitrary `ns0` prefix.
            const seafile = yield* parseFixture("seafile-propfind");
            const directory = nextProp(seafile, 1);
            expect(directory).toMatchObject({
                displayname: "Ma bibliothèque",
                resourcetype: { collection: "" },
            });
            expect(directory.supportedlock).toMatchObject({
                lockentry: [{ lockscope: { exclusive: "" } }, { lockscope: { shared: "" } }],
            });
        }),
    );

    it.effect("preserves extension namespaces, attributes, and text nodes", () =>
        Effect.gen(function* () {
            const attributes = yield* parseFixture("propfind-attributes");
            const props = firstProp(attributes);
            expect(props["z:system-tags"]).toEqual({
                "z:system-tag": [
                    {
                        "@_z:can-assign": "true",
                        "@_z:id": "321",
                        "@_z:checked": "true",
                        text: "Tag1",
                    },
                    { "@_z:can-assign": "false", "@_z:id": "654", "@_z:prop": "", text: "Tag2" },
                ],
            });

            // An attribute and a nested child may share the same local name.
            const conflict = yield* parseFixture("propfind-attributes-conflict");
            expect(firstProp(conflict)["z:prop"]).toEqual({
                "z:link": "text value",
                "@_z:link": "value",
            });
        }),
    );

    it.effect("keeps string, empty, and inline-namespace extension properties", () =>
        Effect.gen(function* () {
            const result = yield* parseFixture("propfind-nextcloud-share-attributes");
            const props = firstProp(result);
            expect(String(props["nc:share-attributes"]).trim()).toBe(
                '[{"scope":"permissions","key":"download","value":false}]',
            );
            expect(props["nc:note"]).toBe("");
            expect(props["oc:size"]).toBe("292842");
            expect(props["x1:share-permissions"]).toBe("31");
        }),
    );

    it.effect("treats an empty multistatus as no responses", () =>
        Effect.gen(function* () {
            const result = yield* parseFixture("empty-multistatus");
            expect(result.multistatus.response).toEqual([]);
        }),
    );

    it.effect("preserves the encoded hrefs the server returned", () =>
        Effect.gen(function* () {
            const entities = yield* parseFixture("propfind-href-html-entities");
            expect(entities.multistatus.response[0]?.href).toBe(
                "http://example.com/files/%26amp%3b.md",
            );

            const query = yield* parseFixture("propfind-href-with-query");
            expect(query.multistatus.response[0]?.href).toBe("/files/some%20file?foo=1&bar=2");
        }),
    );

    it.effect("resolves DAV prefixes from namespace declarations", () =>
        Effect.sync(() => {
            expect([...collectDavPrefixes('<d:multistatus xmlns:d="DAV:">')]).toContain("d");
            expect([...collectDavPrefixes('<ns0:multistatus xmlns:ns0="DAV:">')]).toContain("ns0");
            expect([...collectDavPrefixes("<root />")]).toEqual(["D", "DAV"]);
        }),
    );

    it.effect("rejects malformed XML and unsafe entity declarations", () =>
        Effect.gen(function* () {
            expect((yield* Effect.exit(DavXmlCodecLive.parseGeneric("<root>")))._tag).toBe(
                "Failure",
            );
            expect(
                (yield* Effect.exit(DavXmlCodecLive.parseGeneric("<!DOCTYPE root><root/>")))._tag,
            ).toBe("Failure");
            expect(
                Exit.isFailure(yield* Effect.exit(parseFixture("propfind-numeric-displayname"))),
            ).toBe(false);
        }),
    );
});