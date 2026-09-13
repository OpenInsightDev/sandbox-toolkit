import { Effect, Exit } from "effect";
import { describe, expect, it } from "@effect/vitest";
import { DavXmlCodecLive, ensureDavArray } from "#/xml.ts";

describe("DAV XML codec", () => {
    it.effect("normalizes multistatus responses and known plus extension properties", () =>
        Effect.gen(function* () {
            const result = yield* DavXmlCodecLive.parseMultiStatus(`
        <D:multistatus xmlns:D="DAV:" xmlns:x="urn:example">
          <D:response>
            <D:href>/notes.txt</D:href>
            <D:propstat>
              <D:prop>
                <D:getlastmodified>Mon, 01 Jan 2024 00:00:00 GMT</D:getlastmodified>
                <D:getcontentlength>12</D:getcontentlength>
                <x:color>blue</x:color>
                <D:resourcetype />
              </D:prop>
              <D:status>HTTP/1.1 200 OK</D:status>
            </D:propstat>
          </D:response>
        </D:multistatus>`);

            expect(result.multistatus.response).toHaveLength(1);
            expect(result.multistatus.response[0]).toMatchObject({
                href: "/notes.txt",
                propstat: {
                    status: "HTTP/1.1 200 OK",
                    prop: {
                        getlastmodified: "Mon, 01 Jan 2024 00:00:00 GMT",
                        getcontentlength: "12",
                        "x:color": "blue",
                        resourcetype: {},
                    },
                },
            });
        }),
    );

    it.effect("accepts missing, singleton, and array containers", () =>
        Effect.sync(() => {
            expect(ensureDavArray(undefined)).toEqual([]);
            expect(ensureDavArray(null)).toEqual([]);
            expect(ensureDavArray("one")).toEqual(["one"]);
            expect(ensureDavArray(["one", "two"])).toEqual(["one", "two"]);
        }),
    );

    it.effect("supports Clark notation and builds safe lock XML", () =>
        Effect.gen(function* () {
            const parsed = yield* DavXmlCodecLive.parseGeneric(
                '<D:root xmlns:D="DAV:"><D:item /></D:root>',
                {
                    clarkNotation: true,
                },
            );
            expect(parsed).toHaveProperty("{DAV:}root");

            const lock = yield* DavXmlCodecLive.buildLockInfo(
                "https://dav.example.test/users/alice?x=1&y=2",
            );
            expect(lock).toContain("lockinfo");
            expect(lock).toContain("alice?x=1&amp;y=2");
        }),
    );

    it.effect("does not turn malformed or oversized XML into an empty result", () =>
        Effect.gen(function* () {
            const malformed = yield* Effect.exit(DavXmlCodecLive.parseMultiStatus("<multistatus>"));
            expect(Exit.isFailure(malformed)).toBe(true);
            const oversized = yield* Effect.exit(
                DavXmlCodecLive.parseGeneric("<root>12345</root>", { maxXmlLength: 5 }),
            );
            expect(Exit.isFailure(oversized)).toBe(true);
        }),
    );
});