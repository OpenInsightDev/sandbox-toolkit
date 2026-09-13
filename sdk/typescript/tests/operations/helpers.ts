import { Effect, Layer } from "effect";
import { decodeWebDavConfigSync } from "#/config.ts";
import { AuthTest } from "#/auth.ts";
import { OperationsLive } from "#/operations.ts";
import { responseFromText, TestTransport } from "#/transport.ts";

export const config = decodeWebDavConfigSync({
    remoteUrl: "https://dav.example.test",
    remoteBasePath: "/root",
    authType: "none",
    headers: { "X-Client": "test" },
});

export const makeLayer = (
    execute: Parameters<typeof TestTransport>[0]["execute"],
    input = config,
) => Layer.mergeAll(OperationsLive({ config: input }), AuthTest(input), TestTransport({ execute }));

export const ok = (url: string, status = 204) => responseFromText({ url, status });
export const davResponse = (url: string, body: string, status = 207) =>
    responseFromText({ url, status, body });

export const multistatus = `
<D:multistatus xmlns:D="DAV:" xmlns:x="urn:test">
  <D:response>
    <D:href>/root/</D:href>
    <D:propstat><D:prop><D:resourcetype><D:collection /></D:resourcetype></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat>
  </D:response>
  <D:response>
    <D:href>/root/notes%20one.txt</D:href>
    <D:propstat><D:prop><D:getlastmodified>Mon, 01 Jan 2024 00:00:00 GMT</D:getlastmodified><D:getcontentlength>12</D:getcontentlength><D:getetag>&quot;abc&quot;</D:getetag><D:getcontenttype>text/plain</D:getcontenttype><x:color>blue</x:color></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat>
  </D:response>
</D:multistatus>`;

export const fileMultistatus = `
<D:multistatus xmlns:D="DAV:" xmlns:x="urn:test">
  <D:response><D:href>/root/notes%20one.txt</D:href>
    <D:propstat><D:prop><D:getlastmodified>Mon, 01 Jan 2024 00:00:00 GMT</D:getlastmodified><D:getcontentlength>12</D:getcontentlength><D:getetag>&quot;abc&quot;</D:getetag><D:getcontenttype>text/plain</D:getcontenttype><x:color>blue</x:color></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat>
  </D:response>
</D:multistatus>`;

export const noRequirements = (effect: Effect.Effect<unknown, unknown, never>) => effect;