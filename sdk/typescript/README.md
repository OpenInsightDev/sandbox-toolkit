# WebDAV Client — Effect API

> A WebDAV client for Node.js, browsers, and React Native, built on Effect.

## About

WebDAV is a stable protocol for interacting with remote filesystems over HTTP.
Many file-hosting services — **Nextcloud**/**ownCloud**, **Box**, **Yandex** and others — expose it as a fallback to their primary interfaces.

This package provides a WebDAV client modelled as a set of Effect services.
Every client method returns an `Effect`, so requests compose with the rest of the Effect ecosystem: retries, timeouts, concurrency, streaming, and typed error recovery.

Its API is inspired by the long-standing `webdav` package, but it is not a drop-in replacement:

- **Effects instead of Promises.** Methods never return a `Promise`; you run them with `Effect.runPromise` or any other Effect runner.
- **A layer supplies the runtime.** You provide a `ClientLayer`, which binds the transport, authentication strategy, and available operations together.
- **Streams are Effect streams.** `createReadStream` resolves to a `Stream<Uint8Array>`, and `createWriteStream` accepts one, on every platform.
- **Typed errors.** Failures are tagged values such as `HttpStatusError` and `TransportError`, so you recover with `Effect.catchTag` instead of parsing error messages.

### Supported environments

The client runs anywhere `fetch` is available:

| Environment  | Notes                                           |
| ------------ | ----------------------------------------------- |
| Node.js      | ESM only.                                       |
| Browsers     | Bundle with an ESM-aware tool. Subject to CORS. |
| React Native | Uses the same runtime-agnostic client.          |

The same import works everywhere.
For an explicit runtime entrypoint, the package also exports `NodeLayer`, `BrowserLayer`, and `ReactNativeLayer`, which are currently aliases of `ClientLayer`.

## Installation

```bash
vp add webdav-client
# or
pnpm add webdav-client
# or
npm install webdav-client
```

`effect` is a direct dependency and is installed automatically.

## Usage

Usage has two steps: build a **layer** describing the server, then run an **effect** that yields the client and calls operations on it.

### Creating and running a client

```ts
import { Effect } from "effect";
import { ClientLayer, makeClient } from "webdav-client";

const program = Effect.gen(function* () {
  const client = yield* makeClient();
  return yield* client.getDirectoryContents("/");
}).pipe(
  Effect.provide(
    ClientLayer({
      remoteUrl: "https://dav.example.test/files",
      username: "marie",
      password: "myS3curePa$$w0rd",
    }),
  ),
);

const directoryItems = await Effect.runPromise(program);
// [
//   {
//     filename: "/my-file.txt",
//     basename: "my-file.txt",
//     lastmod: "Mon, 10 Oct 2018 23:24:11 GMT",
//     size: 371,
//     type: "file",
//     etag: null
//   }
// ]
```

The configuration is validated when the layer is created, so an invalid configuration fails immediately.
To validate inside an effect instead, use `decodeWebDavConfig`:

```ts
import { Effect } from "effect";
import { ClientLayer, decodeWebDavConfig, makeClient } from "webdav-client";

const program = Effect.gen(function* () {
  const config = yield* decodeWebDavConfig({ remoteUrl: process.env.DAV_URL });
  return yield* makeClient().pipe(Effect.provide(ClientLayer(config)));
});
```

Layers are values, so build one per server and reuse it:

```ts
const runtime = ClientLayer({
  remoteUrl: "https://dav.example.test/files",
  username: "marie",
  password: "…",
});

const list = Effect.gen(function* () {
  const client = yield* makeClient();
  return yield* client.getDirectoryContents("/");
}).pipe(Effect.provide(runtime));

await Effect.runPromise(list);
```

### Reaching an operation directly

`makeClient` is a convenience facade.
Every operation is also an Effect service, so you can yield one directly when you only need a single capability:

```ts
import { Effect } from "effect";
import { ClientLayer, Stat } from "webdav-client";

const program = Effect.gen(function* () {
  const stat = yield* Stat;
  return yield* stat.execute("/notes.txt");
}).pipe(Effect.provide(ClientLayer({ remoteUrl: "https://dav.example.test/files" })));
```

### Building a client from Promise code

`makeLegacyClient(config)` resolves the client itself, without an Effect context:

```ts
import { makeLegacyClient } from "webdav-client";

const client = await makeLegacyClient({ remoteUrl: "https://dav.example.test/files" });
```

Its methods still return Effects; run each one with `Effect.runPromise`.

## Authentication & Connection

If `authType` is omitted, the client infers it: password authentication when a `username` or `password` is present, otherwise no authentication.
Token and Digest authentication must be requested explicitly.

Setting `authType` manages the `Authorization` header for you.

#### No authentication

Omit `username` and `password` entirely.
No credentials are sent.

#### Basic (password) authentication

Pass `username` and `password`; Basic is selected automatically, or set `authType: "password"` explicitly.

#### Auto-detection

Use `authType: "auto"` when you are unsure whether a server expects Basic or Digest.
The client tries Basic first and switches to Digest if the server asks for it.

#### Digest authentication

```ts
const layer = ClientLayer({
  remoteUrl: "https://address.com",
  authType: "digest",
  username: "someUser",
  password: "myS3curePa$$w0rd",
});
```

#### OAuth / bearer tokens

```ts
const layer = ClientLayer({
  remoteUrl: "https://address.com",
  authType: "token",
  token: {
    access_token: "2YotnFZFEjr1zCsicMWpAA",
    token_type: "Bearer",
    refresh_token: "tGzv3JOkF0XG5Qx2TlKWIA",
  },
});
```

#### Pre-computed Digest HA1

If you generate the HA1 when the user signs in, persist it instead of the password:

```ts
ClientLayer({
  remoteUrl: "https://address.com",
  authType: "digest",
  username: "someUser",
  password: "",
  ha1: "your previously generated ha1 here",
});
```

### Client configuration

| Option            | Default                        | Description                                                                                 |
| ----------------- | ------------------------------ | ------------------------------------------------------------------------------------------- |
| `remoteUrl`       | _required_                     | Full `http`/`https` URL of the WebDAV service. Its path becomes the default request prefix. |
| `remoteBasePath`  | path of `remoteUrl`            | Path prefix to use instead of the one in `remoteUrl`.                                       |
| `authType`        | inferred                       | `"none"`, `"password"`, `"digest"`, `"token"`, or `"auto"`.                                 |
| `username`        | _none_                         | Username for password and digest authentication.                                            |
| `password`        | _none_                         | Password for password and digest authentication.                                            |
| `token`           | _none_                         | Token object (`access_token`, `token_type`, optional `refresh_token`).                      |
| `ha1`             | _none_                         | Pre-computed digest HA1, usable in place of `password`.                                     |
| `headers`         | `{}`                           | Headers added to every request. Method-level headers are merged on top.                     |
| `withCredentials` | `false`                        | Include credentials when making browser requests.                                           |
| `contactHref`     | the project’s lock contact URL | Owner reference embedded in lock requests.                                                  |
| `entityDecoder`   | _none_                         | Limits for XML entity expansion. See [Entity decoder](#entity-decoder).                     |

Validation rules:

- `"password"` requires `username` and `password`.
- `"digest"` requires `username` and either `password` or `ha1`.
- `"token"` requires `token`.
- `"none"` must not include `username` or `password`.
- `"auto"` requires `username` or `password`.

### Entity decoder

The `entityDecoder` option caps XML entity expansion while parsing server responses.
Without it, no limits are applied.

```ts
const layer = ClientLayer({
  remoteUrl: "https://some-server.org",
  username: "user",
  password: "pass",
  entityDecoder: {
    limit: {
      maxTotalExpansions: 1000,
      maxExpandedLength: 50000,
    },
  },
});
```

| Property                   | Default | Description                                                      |
| -------------------------- | ------- | ---------------------------------------------------------------- |
| `limit.maxTotalExpansions` | `0`     | Entity references per document. `0` means unlimited.             |
| `limit.maxExpandedLength`  | `0`     | Characters added by expansion per document. `0` means unlimited. |

Declarations that define their own entities are always rejected.

## Client methods

Each example assumes a `client` obtained from `makeClient()`.
Every method returns an `Effect` whose errors are the `OperationError` union described under [Error handling](#error-handling).

`UploadData` is `string | Uint8Array | ArrayBuffer | Stream<Uint8Array>`.

#### copyFile

Copy an item to another location.

```ts
yield* client.copyFile("/images/source.jpg", "/public/target.jpg");
yield* client.copyFile("/photos", "/backup/photos", { shallow: true, overwrite: false });
```

```ts
(from: string, to: string, options?: CopyFileOptions) => Effect<void, OperationError>;
```

| Argument            | Required | Description                                               |
| ------------------- | -------- | --------------------------------------------------------- |
| `from`              | Yes      | Source path.                                              |
| `to`                | Yes      | Destination path.                                         |
| `options.overwrite` | No       | Replace the destination if it exists. Defaults to `true`. |
| `options.shallow`   | No       | Copy only the item, not the contents of a directory.      |

#### createDirectory

Create a directory.
Creating one that already exists is not an error.

```ts
yield* client.createDirectory("/data/system/storage");
yield* client.createDirectory("/data/system/storage", { recursive: true });
```

```ts
(path: string, options?: CreateDirectoryOptions) => Effect<void, OperationError>;
```

| Argument            | Required | Description                            |
| ------------------- | -------- | -------------------------------------- |
| `path`              | Yes      | The path to create.                    |
| `options.recursive` | No       | Create any missing parent directories. |

#### createReadStream

Create a stream over a remote file.

```ts
import { Effect, Stream } from "effect";

const bytes = yield* client.createReadStream("/video.mp4").pipe(
  Effect.flatMap(Stream.runCollect),
  Effect.map((chunks) => Uint8Array.from(chunks.flatMap((chunk) => [...chunk]))),
);
```

Stream a byte range:

```ts
const head = yield* client.createReadStream("/video.mp4", {
  range: { start: 0, end: 1024 },
});
```

```ts
(filename: string, options?: CreateReadStreamOptions) =>
  Effect<Stream<Uint8Array, OperationError>, OperationError>;
```

| Argument              | Required | Description                     |
| --------------------- | -------- | ------------------------------- |
| `filename`            | Yes      | The remote file to stream.      |
| `options.range.start` | Yes*     | First byte position, inclusive. |
| `options.range.end`   | No       | Last byte position, inclusive.  |

_\*Required only when a `range` is given._

The stream is released automatically on completion, failure, or interruption.

#### createWriteStream

Write a stream to a remote file.

```ts
yield* client.createWriteStream("/music/song.mp3", Stream.fromIterable([bytes]));
```

```ts
(filename: string, input: Stream<Uint8Array, OperationError>, options?: CreateWriteStreamOptions) =>
  Effect<boolean, OperationError>;
```

| Argument            | Required | Description                                                          |
| ------------------- | -------- | -------------------------------------------------------------------- |
| `filename`          | Yes      | The remote file to write.                                            |
| `input`             | Yes      | The bytes to write, as an Effect stream.                             |
| `options.overwrite` | No       | Refuse to replace an existing file when `false`. Defaults to `true`. |

Returns `false` when writing was refused, `true` otherwise.

#### customRequest

Send a request directly, with the client’s authentication and configuration applied.
The URL must include the scheme and host.

```ts
const response = yield* client.customRequest({
  url: "https://dav.example.test/files/alrighty.jpg",
  method: "PROPFIND",
  headers: { Accept: "text/plain,application/xml", Depth: "0" },
});
// { status: 207, statusText: "Multi-Status", url: "...", headers: { ... }, body: "..." }
```

```ts
(request: {
  url: string;
  method: string;
  headers?: Headers;
  data?: string | Uint8Array | ArrayBuffer | Record<string, unknown>;
}) => Effect<DetailedHttpResponse, OperationError>;
```

| Argument  | Required | Description                                                    |
| --------- | -------- | -------------------------------------------------------------- |
| `url`     | Yes      | Full request URL.                                              |
| `method`  | Yes      | Any HTTP method, including extension methods such as `REPORT`. |
| `headers` | No       | Request headers.                                               |
| `data`    | No       | Request body.                                                  |

The response `body` is text.

#### deleteFile

Delete a remote file or directory.
Deleting something that does not exist is not an error.

```ts
yield* client.deleteFile("/tmp.dat");
```

```ts
(filename: string, options?: MethodOptions) => Effect<void, OperationError>;
```

#### exists

Check whether a path exists.
An authentication failure is reported as `false` rather than as an error.

```ts
if (!(yield* client.exists("/some/path"))) {
  yield* client.createDirectory("/some/path");
}
```

```ts
(path: string, options?: MethodOptions) => Effect<boolean, OperationError>;
```

#### getDAVCompliance

Report the DAV compliance classes and server name advertised by the server.

```ts
const compliance = yield* client.getDAVCompliance("/");
// { compliance: ["1", "2"], server: "rshs" }
```

```ts
(path?: string, options?: MethodOptions) => Effect<DAVCompliance, OperationError>;
```

| Argument | Required | Description                          |
| -------- | -------- | ------------------------------------ |
| `path`   | No       | Path to query; defaults to the root. |

#### getDirectoryContents

List a collection, excluding the collection itself unless requested.

```ts
const contents = yield* client.getDirectoryContents("/");
const all = yield* client.getDirectoryContents("/", { deep: true });
const images = yield* client.getDirectoryContents("/", {
  deep: true,
  glob: "/photos/**/*.{png,jpg,gif}",
});
```

```ts
(path: string, options?: GetDirectoryContentsOptions) =>
  Effect<ReadonlyArray<FileStat>, OperationError>;
```

| Argument              | Required | Description                                                             |
| --------------------- | -------- | ----------------------------------------------------------------------- |
| `path`                | Yes      | Collection to list.                                                     |
| `options.deep`        | No       | List the whole subtree instead of direct children. Defaults to `false`. |
| `options.glob`        | No       | Keep only results whose path matches the pattern.                       |
| `options.includeSelf` | No       | Include the collection’s own entry. Defaults to `false`.                |
| `options.details`     | No       | Accepted for compatibility; ignored.                                    |

Names are decoded, so results match the names as they appear on the server even when they contain spaces, `%`, `&`, or non-ASCII characters.

#### getFileContents

Fetch a remote file.

```ts
const bytes: Uint8Array = yield* client.getFileContents("/package.zip");
const text: string = yield* client.getFileContents("/config.json", { format: "text" });
const detailed = yield* client.getFileContents("/photo.jpg", { details: true });
```

```ts
(filename: string, options?: GetFileContentsOptions) =>
  Effect<Uint8Array | string | DetailedHttpResponse, OperationError>;
```

| Argument          | Required | Description                                                    |
| ----------------- | -------- | -------------------------------------------------------------- |
| `filename`        | Yes      | The file to fetch.                                             |
| `options.format`  | No       | `"binary"` (default) returns bytes; `"text"` returns a string. |
| `options.details` | No       | Return a `DetailedHttpResponse` instead of the body alone.     |

When `details` is set, `body` is always returned as bytes.
Use [`createReadStream`](#createreadstream) for large files.

#### getFileDownloadLink / getFileUploadLink

Describe the request that would download or upload a path.
These methods never embed credentials, so the URL and headers are safe to share.

```ts
const download = yield* client.getFileDownloadLink("/image.png");
// { url: "https://dav.example.test/files/image.png", method: "GET", headers: {} }

const upload = yield* client.getFileUploadLink("/image.png");
// { url: "https://dav.example.test/files/image.png", method: "PUT", headers: { "Content-Type": "application/octet-stream" } }
```

```ts
(path: string) => Effect<FileDownloadLink, OperationError>
(path: string) => Effect<FileUploadLink, OperationError>
```

| Argument | Required | Description              |
| -------- | -------- | ------------------------ |
| `path`   | Yes      | Remote path to describe. |

If the server requires authentication, add the appropriate header yourself.

#### getQuota

Read the account quota.
Returns `null` when the server does not provide one.

```ts
const quota = yield* client.getQuota();
// { used: 1938743, available: "unlimited" }

const atPath = yield* client.getQuota({ path: "/sub1" });
```

```ts
(options?: GetQuotaOptions) => Effect<DiskQuota | null, OperationError>;
```

| Argument          | Required | Description                          |
| ----------------- | -------- | ------------------------------------ |
| `options.path`    | No       | Path the request targets.            |
| `options.details` | No       | Accepted for compatibility; ignored. |

`available` is a byte count, `"unlimited"`, or `"unknown"`.

#### lock

Acquire a write lock.

```ts
const lock = yield* client.lock("/file.doc", { timeout: "Second-3600" });
yield* client.unlock("/file.doc", lock.token);

// Refresh an existing lock:
yield* client.lock("/file.doc", { refreshToken: lock.token });
```

```ts
(path: string, options?: LockOptions) => Effect<LockResponse, OperationError>;
```

| Argument               | Required | Description                |
| ---------------------- | -------- | -------------------------- |
| `path`                 | Yes      | Resource to lock.          |
| `options.timeout`      | No       | Requested lock duration.   |
| `options.refreshToken` | No       | Existing token to refresh. |

The result is `{ token, serverTimeout }`.

#### moveFile

Move an item.

```ts
yield* client.moveFile("/file1.png", "/file2.png");
yield* client.moveFile("/a", "/b", { overwrite: false });
```

```ts
(from: string, to: string, options?: MoveFileOptions) => Effect<void, OperationError>;
```

| Argument            | Required | Description                                               |
| ------------------- | -------- | --------------------------------------------------------- |
| `from`              | Yes      | Source path.                                              |
| `to`                | Yes      | Destination path.                                         |
| `options.overwrite` | No       | Replace the destination if it exists. Defaults to `true`. |

#### partialUpdateFileContents

Partially update a file using the HTTP PATCH method (RFC 5789).

**With byte range (uses PATCH):**

```ts
// Update bytes 10-20 of the file
yield* client.partialUpdateFileContents("/file.txt", "new content", {
  range: { start: 10, end: 20 }
});

// Append from position 100 onwards
yield* client.partialUpdateFileContents("/file.txt", "appended data", {
  range: { start: 100 }
});

// With custom content type
yield* client.partialUpdateFileContents("/data.json", '{"key":"value"}', {
  range: { start: 0, end: 14 },
  contentType: "application/json"
});
```

**Without range (falls back to PUT):**

```ts
// Replaces entire file
yield* client.partialUpdateFileContents("/log.txt", chunk);
```

```ts
(path: string, data: UploadData, options?: PartialUpdateOptions) => Effect<boolean, OperationError>;
```

**Options:**

- `range?: { start: number; end?: number }` - Byte range to update. If `end` is omitted, it's calculated as `start + dataLength - 1`
- `contentType?: string` - Content type of the patch data (default: `"application/octet-stream"`)
- `headers?: Headers` - Additional HTTP headers
- `signal?: AbortSignal` - Abort signal for cancellation

**Error handling:**

- Returns `UnsupportedFeatureError` if the server responds with 409 Conflict (PATCH not supported)
- Accepts 200, 204, or 206 status codes as success

**Note:** Without a `range`, this operation falls back to a full PUT request and replaces the entire file.

#### putFileContents

Write data to a remote file.
Returns `false` when the write was refused, `true` otherwise.

```ts
yield* client.putFileContents("/my/file.jpg", imageBytes, { overwrite: false });
yield* client.putFileContents("/my/file.txt", "hello");
yield* client.putFileContents("/my/stream.bin", Stream.fromIterable([chunkA, chunkB]));
```

```ts
(filename: string, data: UploadData, options?: PutFileContentsOptions) =>
  Effect<boolean, OperationError>;
```

| Argument                | Required | Description                                                                 |
| ----------------------- | -------- | --------------------------------------------------------------------------- |
| `filename`              | Yes      | File to write.                                                              |
| `data`                  | Yes      | `string`, `Uint8Array`, `ArrayBuffer`, or `Stream<Uint8Array>`.             |
| `options.contentLength` | No       | `true` (default) calculates the length, `false` omits it, a number sets it. |
| `options.overwrite`     | No       | When `false`, refuse to replace an existing file. Defaults to `true`.       |

#### search

Run a WebDAV search as per [RFC 5323](https://www.ietf.org/rfc/rfc5323.html).

```ts
const searchRequest = `<?xml version="1.0" encoding="UTF-8"?>
<d:searchrequest xmlns:d="DAV:" xmlns:f="http://example.com/foo">
  <f:natural-language-query>
  Find files changed last week
  </f:natural-language-query>
</d:searchrequest>`;

const result = yield* client.search("/some-collection", { data: searchRequest });
// { truncated: false, results: [ … ] }
```

```ts
(path: string, options?: SearchOptions) => Effect<SearchResult, OperationError>;
```

| Argument          | Required | Description                          |
| ----------------- | -------- | ------------------------------------ |
| `path`            | Yes      | Collection the search runs against.  |
| `options.data`    | Yes*     | The search request body.             |
| `options.details` | No       | Accepted for compatibility; ignored. |

_\*Optional in the type, but a search without a body is rarely useful._

`truncated` is `true` when the server reports that it could not return everything.
Results include every item the server returned, which may include the searched collection itself.

#### stat

Fetch a single item’s details.

```ts
const stat = yield* client.stat("/some/file.tar.gz");
// { filename, basename, lastmod, size, type, etag, mime?, props? }
```

```ts
(path: string, options?: StatOptions) => Effect<FileStat, OperationError>;
```

| Argument          | Required | Description                                                                         |
| ----------------- | -------- | ----------------------------------------------------------------------------------- |
| `path`            | Yes      | Remote path to inspect.                                                             |
| `options.details` | No       | Accepted for compatibility; ignored. Server-provided properties are still returned. |

#### unlock

Release a lock using its token.

```ts
yield* client.unlock("/file.doc", lock.token);
```

```ts
(path: string, token: string, options?: MethodOptions) => Effect<void, OperationError>;
```

| Argument | Required | Description                         |
| -------- | -------- | ----------------------------------- |
| `path`   | Yes      | Remote path to unlock.              |
| `token`  | Yes      | Token from a previous lock request. |

### Method options

Most operations accept a `MethodOptions` object:

| Option    | Required | Description                                                                               |
| --------- | -------- | ----------------------------------------------------------------------------------------- |
| `data`    | No       | Body to send. For read-style operations this replaces the request body the client builds. |
| `headers` | No       | Additional headers, merged over the client’s headers and the method defaults.             |
| `signal`  | No       | An `AbortSignal` for cancellation; aborting fails with `AbortError`.                      |

Each operation extends these with its own fields, listed with the method.

## Composing with Effect

Because every method is an `Effect`, the usual combinators apply.

**Recover from a specific failure.**

```ts
const result = yield* client.stat("/maybe.txt").pipe(
  Effect.catchTag("HttpStatusError", (error) =>
    Effect.succeed({ missing: error.status === 404 }),
  ),
);
```

**Retry transient failures.**

```ts
import { Schedule } from "effect";

const bytes = yield* client.getFileContents("/report.pdf").pipe(
  Effect.retry({
    schedule: Schedule.exponential("200 millis"),
    while: (error) => error._tag === "TransportError",
  }),
);
```

**Bound the wait.**

```ts
const bytes = yield* client.getFileContents("/report.pdf").pipe(Effect.timeout("10 seconds"));
```

**Run requests concurrently.**

```ts
const stats = yield* Effect.all(
  paths.map((path) => client.stat(path)),
  { concurrency: 8 },
);
```

**Pipe one file into another.**

```ts
const copied = yield* Effect.gen(function* () {
  const stream = yield* client.createReadStream("/source.bin");
  return yield* client.createWriteStream("/backup.bin", stream);
});
```

## Common data structures

### Item stats (`FileStat`)

```json
{
  "filename": "/image.jpg",
  "basename": "image.jpg",
  "lastmod": "Sun, 13 Mar 2016 04:23:32 GMT",
  "size": 42497,
  "type": "file",
  "etag": "\"33a728c7f288ede1fecc90ac6a10e062\"",
  "mime": "image/jpeg",
  "props": { "getetag": "\"33a…\"", "getlastmodified": "…" }
}
```

| Property   | Type                    | Present       | Description                                           |
| ---------- | ----------------------- | ------------- | ----------------------------------------------------- |
| `filename` | `string`                | Always        | Path of the remote item.                              |
| `basename` | `string`                | Always        | Final path segment, decoded.                          |
| `lastmod`  | `string`                | Always        | Last modification date; empty if the server omits it. |
| `size`     | `number`                | Always        | Byte size; `0` for directories.                       |
| `type`     | `"file" \| "directory"` | Always        | Item type.                                            |
| `etag`     | `string \| null`        | Always        | Entity tag, or `null` when absent.                    |
| `mime`     | `string`                | When provided | Content type, for files.                              |
| `props`    | object                  | When provided | All server-provided properties.                       |

### Detailed responses (`DetailedHttpResponse`)

Returned by `getFileContents({ details: true })` and `customRequest`:

| Property     | Type                   | Description          |
| ------------ | ---------------------- | -------------------- |
| `status`     | `number`               | Numeric status code. |
| `statusText` | `string`               | Status text.         |
| `url`        | `string`               | Final response URL.  |
| `headers`    | object                 | Response headers.    |
| `body`       | `string \| Uint8Array` | Response body.       |

### Quota (`DiskQuota`)

```ts
{ used: 1938743, available: "unlimited" } // available: number | "unlimited" | "unknown"
```

### Search (`SearchResult`)

```ts
{ truncated: false, results: [ /* FileStat */ ] }
```

### DAV compliance (`DAVCompliance`)

```ts
{ compliance: ["1", "2"], server: "rshs" }
```

### Lock (`LockResponse`)

```ts
{ token: "opaquelocktoken:…", serverTimeout: "Second-3600" }
```

## Error handling

Failures are tagged values with a `_tag` discriminator.
Recover from a specific one with `Effect.catchTag`, or handle everything with `Effect.catch`.

```ts
yield* client.deleteFile("/protected.txt").pipe(
  Effect.catchTag("AuthenticationError", () => Effect.void),
  Effect.catchTag("HttpStatusError", (error) =>
    error.status === 403 ? Effect.void : Effect.fail(error),
  ),
);
```

The errors a WebDAV operation can produce form the `OperationError` union:

`HttpStatusError`, `InvalidDavResponseError`, `ResponseDecodeError`, `XmlParseError`, `XmlEncodeError`, `StreamError`, `TransportError`, `AbortError`, and `AuthenticationError`.

Configuration problems are reported separately as `InvalidConfigError`.

Every error carries an `operation` and a `message`, and — where relevant — a `url` and `status`.

## CORS

When running in a browser, the server must return appropriate CORS headers.
Some servers (for example Nextcloud by default) do not, which prevents browser usage unless the server is configured to allow it.

## Security

`getFileDownloadLink` and `getFileUploadLink` return request descriptions without embedding passwords, tokens, or Basic credentials in URLs.
If you need authenticated access, attach the `Authorization` header from your own auth flow rather than placing credentials in the URL.

## Development

```bash
vp install
vp check
vp test
vp pack
```

The end-to-end tests run against a real WebDAV server in Docker.
They are skipped unless `WEBDAV_E2E=1` is set and a Docker daemon is available:

```bash
vp run test:e2e
```
