import { Context, Effect, Layer, Schema, Stream } from "effect";
import {
    CopyFileOptions,
    CreateDirectoryOptions,
    GetDirectoryContentsOptions,
    GetFileContentsOptions,
    GetQuotaOptions,
    LockOptions,
    MoveFileOptions,
    PartialUpdateOptions,
    PutFileContentsOptions,
    CreateReadStreamOptions,
    CreateWriteStreamOptions,
    SearchOptions,
    StatOptions,
    type NormalizedWebDavConfig,
} from "#/config.ts";
import type {
    DAVCompliance,
    DAVProperties,
    DiskQuota,
    FileStat,
    Headers,
    LockResponse,
    SearchResult,
} from "#/domain.ts";
import {
    DAVCompliance as DAVComplianceSchema,
    DiskQuota as DiskQuotaSchema,
    FileStat as FileStatSchema,
    SearchResult as SearchResultSchema,
} from "#/domain.ts";
import {
    AbortError,
    HttpStatusError,
    InvalidDavResponseError,
    ResponseDecodeError,
    TransportError,
    XmlParseError,
    XmlEncodeError,
    StreamError,
    AuthenticationError,
    UnsupportedFeatureError,
} from "#/error.ts";
import { Transport, type WebDavHttpResponse } from "#/transport.ts";
import { DavXmlCodecLive, type XmlParseOptions } from "#/xml.ts";
import { getHeader } from "#/utils/headers.ts";
import { authenticatedRequest, type Auth } from "./auth.ts";

type StatInput = Schema.Schema.Type<typeof StatOptions>;
type SearchInput = Schema.Schema.Type<typeof SearchOptions>;
type GetQuotaInput = Schema.Schema.Type<typeof GetQuotaOptions>;
type GetDirectoryContentsInput = Schema.Schema.Type<typeof GetDirectoryContentsOptions>;
type CreateDirectoryInput = Schema.Schema.Type<typeof CreateDirectoryOptions>;
type MoveFileInput = Schema.Schema.Type<typeof MoveFileOptions>;
type CopyFileInput = Schema.Schema.Type<typeof CopyFileOptions>;
type GetFileContentsInput = Schema.Schema.Type<typeof GetFileContentsOptions>;
type PutFileContentsInput = Schema.Schema.Type<typeof PutFileContentsOptions>;
type PartialUpdateInput = Schema.Schema.Type<typeof PartialUpdateOptions>;
type CreateReadStreamInput = Schema.Schema.Type<typeof CreateReadStreamOptions>;
type CreateWriteStreamInput = Schema.Schema.Type<typeof CreateWriteStreamOptions>;
type LockInput = Schema.Schema.Type<typeof LockOptions>;

export type UploadData = string | Uint8Array | ArrayBuffer | Stream.Stream<Uint8Array, unknown>;
export type FileContents = string | Uint8Array;
export interface FileDownloadLink {
    readonly url: string;
    readonly method: "GET";
    readonly headers: Headers;
}
export interface FileUploadLink {
    readonly url: string;
    readonly method: "PUT";
    readonly headers: Headers;
}

export interface DetailedHttpResponse {
    readonly status: number;
    readonly statusText: string;
    readonly url: string;
    readonly headers: Headers;
    readonly body: string | Uint8Array;
}

export interface DeleteFile {
    readonly execute: (
        path: string,
        options?: Record<string, unknown>,
    ) => Effect.Effect<void, OperationError, OperationRequirements>;
}
export const DeleteFile = Context.Service<DeleteFile>("webdav/DeleteFile");
export interface Exists {
    readonly execute: (
        path: string,
        options?: Record<string, unknown>,
    ) => Effect.Effect<boolean, OperationError, OperationRequirements>;
}
export const Exists = Context.Service<Exists>("webdav/Exists");
export interface MoveFile {
    readonly execute: (
        from: string,
        to: string,
        options?: MoveFileInput,
    ) => Effect.Effect<void, OperationError, OperationRequirements>;
}
export const MoveFile = Context.Service<MoveFile>("webdav/MoveFile");
export interface CopyFile {
    readonly execute: (
        from: string,
        to: string,
        options?: CopyFileInput,
    ) => Effect.Effect<void, OperationError, OperationRequirements>;
}
export const CopyFile = Context.Service<CopyFile>("webdav/CopyFile");
export interface CreateDirectory {
    readonly execute: (
        path: string,
        options?: CreateDirectoryInput,
    ) => Effect.Effect<void, OperationError, OperationRequirements>;
}
export const CreateDirectory = Context.Service<CreateDirectory>("webdav/CreateDirectory");
export interface GetDavCompliance {
    readonly execute: (
        path?: string,
        options?: Record<string, unknown>,
    ) => Effect.Effect<DAVCompliance, OperationError, OperationRequirements>;
}
export const GetDavCompliance = Context.Service<GetDavCompliance>("webdav/GetDavCompliance");
export interface GetQuota {
    readonly execute: (
        options?: GetQuotaInput,
    ) => Effect.Effect<DiskQuota | null, OperationError, OperationRequirements>;
}
export const GetQuota = Context.Service<GetQuota>("webdav/GetQuota");
export interface Stat {
    readonly execute: (
        path: string,
        options?: StatInput,
    ) => Effect.Effect<FileStat, OperationError, OperationRequirements>;
}
export const Stat = Context.Service<Stat>("webdav/Stat");
export interface GetDirectoryContents {
    readonly execute: (
        path: string,
        options?: GetDirectoryContentsInput,
    ) => Effect.Effect<ReadonlyArray<FileStat>, OperationError, OperationRequirements>;
}
export const GetDirectoryContents = Context.Service<GetDirectoryContents>(
    "webdav/GetDirectoryContents",
);
export interface Search {
    readonly execute: (
        path: string,
        options?: SearchInput,
    ) => Effect.Effect<SearchResult, OperationError, OperationRequirements>;
}
export const Search = Context.Service<Search>("webdav/Search");
export interface CustomRequest {
    readonly execute: (request: {
        readonly url: string;
        readonly method: string;
        readonly headers?: Headers;
        readonly data?: unknown;
    }) => Effect.Effect<DetailedHttpResponse, OperationError, OperationRequirements>;
}
export const CustomRequest = Context.Service<CustomRequest>("webdav/CustomRequest");
export interface GetFileContents {
    readonly execute: (
        path: string,
        options?: GetFileContentsInput,
    ) => Effect.Effect<FileContents | DetailedHttpResponse, OperationError, OperationRequirements>;
}
export const GetFileContents = Context.Service<GetFileContents>("webdav/GetFileContents");
export interface PutFileContents {
    readonly execute: (
        path: string,
        data: UploadData,
        options?: PutFileContentsInput,
    ) => Effect.Effect<boolean, OperationError, OperationRequirements>;
}
export const PutFileContents = Context.Service<PutFileContents>("webdav/PutFileContents");
export interface PartialUpdateFileContents {
    readonly execute: (
        path: string,
        data: UploadData,
        options?: PartialUpdateInput,
    ) => Effect.Effect<boolean, OperationError, OperationRequirements>;
}
export const PartialUpdateFileContents = Context.Service<PartialUpdateFileContents>(
    "webdav/PartialUpdateFileContents",
);
export interface CreateReadStream {
    readonly execute: (
        path: string,
        options?: CreateReadStreamInput,
    ) => Effect.Effect<
        Stream.Stream<Uint8Array, OperationError>,
        OperationError,
        OperationRequirements
    >;
}
export const CreateReadStream = Context.Service<CreateReadStream>("webdav/CreateReadStream");
export interface CreateWriteStream {
    readonly execute: (
        path: string,
        input: Stream.Stream<Uint8Array, OperationError>,
        options?: CreateWriteStreamInput,
    ) => Effect.Effect<boolean, OperationError, OperationRequirements>;
}
export const CreateWriteStream = Context.Service<CreateWriteStream>("webdav/CreateWriteStream");
export interface Lock {
    readonly execute: (
        path: string,
        options?: LockInput,
    ) => Effect.Effect<LockResponse, OperationError, OperationRequirements>;
}
export const Lock = Context.Service<Lock>("webdav/Lock");
export interface Unlock {
    readonly execute: (
        path: string,
        token: string,
        options?: OperationOptions,
    ) => Effect.Effect<void, OperationError, OperationRequirements>;
}
export const Unlock = Context.Service<Unlock>("webdav/Unlock");
export interface FileLinks {
    readonly download: (path: string) => Effect.Effect<FileDownloadLink, OperationError>;
    readonly upload: (path: string) => Effect.Effect<FileUploadLink, OperationError>;
}
export const FileLinks = Context.Service<FileLinks>("webdav/FileLinks");

export type OperationError =
    | HttpStatusError
    | InvalidDavResponseError
    | ResponseDecodeError
    | XmlParseError
    | XmlEncodeError
    | StreamError
    | TransportError
    | AbortError
    | AuthenticationError
    | UnsupportedFeatureError;

export type OperationRequirements = Auth | Transport;

type OperationOptions = {
    readonly headers?: Headers;
    readonly signal?: AbortSignal;
    readonly data?: unknown;
};

const textDecoder = new TextDecoder();
const error = (operation: string, message: string, response?: WebDavHttpResponse) =>
    new HttpStatusError({
        operation,
        message,
        ...(response === undefined ? {} : { status: response.status, url: response.url }),
    });
const invalid = (operation: string, message: string, url?: string) =>
    new InvalidDavResponseError({ operation, message, ...(url === undefined ? {} : { url }) });

/** Extract the numeric code from a DAV status line such as `HTTP/1.1 404 Not Found`. */
const statusCode = (statusLine?: string): number | undefined => {
    if (!statusLine) return undefined;
    const match = /\s(\d{3})\s/.exec(` ${statusLine} `);
    return match ? Number(match[1]) : undefined;
};

/** Translate the `quota-available-bytes` sentinel values documented by RFC 4331. */
const translateQuotaAvailable = (value: unknown): number | "unknown" | "unlimited" => {
    switch (String(value)) {
        case "-3":
            return "unlimited";
        case "-2":
        case "-1":
            return "unknown";
        default: {
            if (value === "" || value === undefined || value === null) return "unknown";
            const parsed = Number(value);
            return Number.isFinite(parsed) ? parsed : "unknown";
        }
    }
};
const encodePathSegments = (value: string): string =>
    value
        .split("/")
        .map((segment) => encodeURIComponent(segment))
        .join("/");
/**
 * Build an absolute request URL. The path prefix is the explicit `remoteBasePath`
 * when provided, otherwise the path of `remoteUrl`; each path segment is
 * percent-encoded so names containing spaces, `%`, `&`, or non-ASCII characters
 * resolve to the intended resource instead of being misinterpreted by the server.
 */
const pathUrl = (config: NormalizedWebDavConfig, path = ""): string => {
    const root = new URL(config.remoteUrl);
    const rawBase = config.remoteBasePath;
    const base =
        rawBase === undefined
            ? root.pathname.replace(/\/+$/, "")
            : encodePathSegments(`/${rawBase.replace(/^\/+|\/+$/g, "")}`).replace(/\/+$/, "");
    const suffix = encodePathSegments(path.replace(/^\/+/, ""));
    const joined = `${base}/${suffix}`;
    root.pathname = joined.startsWith("/") ? joined : `/${joined}`;
    return root.toString();
};
const requestOptions = (config: NormalizedWebDavConfig, options?: OperationOptions) => ({
    headers: { ...config.headers, ...options?.headers },
    ...(options?.data === undefined ? {} : { data: options.data as never }),
    ...(options?.signal === undefined ? {} : { signal: options.signal }),
    withCredentials: config.withCredentials,
});
const statusOk = (
    response: WebDavHttpResponse,
    operation: string,
    statuses: ReadonlySet<number>,
) =>
    statuses.has(response.status)
        ? Effect.succeed(response)
        : Effect.fail(error(operation, `Unexpected HTTP status ${response.status}`, response));
const bodyBytes = (response: WebDavHttpResponse): Effect.Effect<Uint8Array, OperationError> =>
    Stream.runCollect(response.stream).pipe(
        Effect.map((chunks) => Uint8Array.from(chunks.flatMap((chunk) => [...chunk]))),
        Effect.mapError((cause) =>
            cause instanceof AbortError || cause instanceof TransportError
                ? cause
                : new StreamError({
                      operation: "response",
                      message: "Response stream failed",
                      cause: String(cause),
                  }),
        ),
    );
const bodyText = (response: WebDavHttpResponse): Effect.Effect<string, OperationError> =>
    Stream.runCollect(response.stream).pipe(
        Effect.map((chunks) =>
            textDecoder.decode(Uint8Array.from(chunks.flatMap((chunk) => [...chunk]))),
        ),
        Effect.mapError((cause) =>
            cause instanceof AbortError || cause instanceof TransportError
                ? cause
                : new StreamError({
                      operation: "response",
                      message: "Response stream failed",
                      cause: String(cause),
                  }),
        ),
    );
const xmlOptions = (config: NormalizedWebDavConfig): XmlParseOptions =>
    config.entityDecoder === undefined ? {} : { entityDecoder: config.entityDecoder };
const parse = (config: NormalizedWebDavConfig, response: WebDavHttpResponse) =>
    bodyText(response).pipe(
        Effect.flatMap((xml) => DavXmlCodecLive.parseMultiStatus(xml, xmlOptions(config))),
        Effect.mapError((cause) => cause),
    );
const property = (stat: { readonly prop?: DAVProperties }, name: string) => stat.prop?.[name];
const decodeHref = (value: string): string => {
    try {
        return decodeURIComponent(value);
    } catch {
        return value;
    }
};
/** Strip an absolute origin so hrefs are comparable regardless of how the server serialises them. */
const normaliseHref = (href: string): string => href.replace(/^https?:\/\/[^/]+/i, "");
/** Reconstruct the client-relative filename, decoding percent escapes and trimming a trailing slash. */
const filenameFromHref = (href: string): string => {
    const value = decodeHref(normaliseHref(href));
    const absolute = value.startsWith("/") ? value : `/${value}`;
    return absolute.length > 1 ? absolute.replace(/\/+$/, "") : absolute;
};
const basenameOf = (filename: string): string => {
    const trimmed = filename.replace(/\/+$/, "");
    return trimmed.split("/").pop() ?? "";
};

const formatLineRange = (range: { readonly start: number; readonly end?: number }): string =>
    `lines=${range.start}-${range.end === undefined ? "" : range.end}`;

const formatByteRange = (range: { readonly start: number; readonly end?: number }): string =>
    `bytes=${range.start}-${range.end === undefined ? "" : range.end}`;
const fileStat = (href: string, prop?: DAVProperties): FileStat => {
    const resource = property({ prop }, "resourcetype");
    const length = property({ prop }, "getcontentlength");
    const size = typeof length === "number" ? length : Number(length ?? 0);
    const filename = filenameFromHref(href);
    return Schema.decodeUnknownSync(FileStatSchema)({
        filename,
        basename: basenameOf(filename),
        lastmod:
            typeof property({ prop }, "getlastmodified") === "string"
                ? property({ prop }, "getlastmodified")
                : "",
        size: Number.isFinite(size) ? size : 0,
        type:
            resource && typeof resource === "object" && "collection" in resource
                ? "directory"
                : "file",
        etag:
            property({ prop }, "getetag") === undefined
                ? null
                : String(property({ prop }, "getetag")),
        ...(property({ prop }, "getcontenttype") === undefined
            ? {}
            : { mime: String(property({ prop }, "getcontenttype")) }),
        ...(prop === undefined ? {} : { props: prop }),
    });
};

const makeLive = (config: NormalizedWebDavConfig) => {
    const execute = (request: Parameters<Transport["execute"]>[0]) => authenticatedRequest(request);
    const simple = (
        method: string,
        operation: string,
        statuses: ReadonlySet<number>,
        path: string,
        options?: OperationOptions,
        extra?: Headers,
    ) =>
        execute({
            url: pathUrl(config, path),
            method,
            ...requestOptions(config, options),
            headers: { ...requestOptions(config, options).headers, ...extra },
        }).pipe(
            Effect.flatMap((response) => statusOk(response, operation, statuses)),
            Effect.asVoid,
        );
    const stat = (path: string, options: StatInput = {}) => {
        const headers = { Depth: "0", "Content-Type": "application/xml", ...options.headers };
        return execute({
            url: pathUrl(config, path),
            method: "PROPFIND",
            ...requestOptions(config, options),
            headers,
        }).pipe(
            Effect.flatMap((response) => statusOk(response, "stat", new Set([200, 207]))),
            Effect.flatMap((response) => parse(config, response)),
            Effect.flatMap((result): Effect.Effect<FileStat, OperationError> => {
                const item = result.multistatus.response[0];
                if (!item) {
                    return Effect.fail(
                        invalid("stat", "DAV response contains no resource", pathUrl(config, path)),
                    );
                }
                const code = statusCode(item.propstat?.status ?? item.status);
                if (code !== undefined && code >= 400) {
                    return Effect.fail(
                        new HttpStatusError({
                            operation: "stat",
                            message: `Unexpected DAV status ${code}`,
                            status: code,
                            url: pathUrl(config, path),
                        }),
                    );
                }
                return Effect.succeed(fileStat(item.href, item.propstat?.prop));
            }),
        );
    };
    const getContents = (path: string, options: GetDirectoryContentsInput = {}) => {
        const depth = options.deep ? "infinity" : "1";
        const directory = path.endsWith("/") ? path : `${path}/`;
        return execute({
            url: pathUrl(config, directory),
            method: "PROPFIND",
            ...requestOptions(config, options),
            headers: { Depth: depth, "Content-Type": "application/xml", ...options.headers },
        }).pipe(
            Effect.flatMap((response) =>
                statusOk(response, "getDirectoryContents", new Set([200, 207])),
            ),
            Effect.flatMap((response) => parse(config, response)),
            Effect.map((result) =>
                result.multistatus.response
                    .filter((item, index) => options.includeSelf || index > 0)
                    .map((item) => fileStat(item.href, item.propstat?.prop))
                    .filter((item) => !options.glob || globMatch(options.glob, item.filename)),
            ),
        );
    };
    const uploadBytes = (data: UploadData): Effect.Effect<Uint8Array, OperationError> =>
        data instanceof Uint8Array
            ? Effect.succeed(data)
            : data instanceof ArrayBuffer
              ? Effect.succeed(new Uint8Array(data))
              : typeof data === "string"
                ? Effect.succeed(new TextEncoder().encode(data))
                : Stream.runCollect(data).pipe(
                      Effect.map((chunks) =>
                          Uint8Array.from(chunks.flatMap((chunk) => [...chunk])),
                      ),
                      Effect.mapError(
                          (cause) =>
                              new StreamError({
                                  operation: "upload",
                                  message: "Upload stream failed",
                                  cause: String(cause),
                              }),
                      ),
                  );
    const getFileContents = (
        path: string,
        options: GetFileContentsInput = {},
    ): Effect.Effect<
        FileContents | DetailedHttpResponse,
        OperationError,
        OperationRequirements
    > => {
        const range =
            options.lineRange === undefined
                ? options.range === undefined
                    ? undefined
                    : options.range.unit === "lines"
                      ? formatLineRange(options.range)
                      : formatByteRange(options.range)
                : formatLineRange(options.lineRange);
        return execute({
            url: pathUrl(config, path),
            method: "GET",
            ...requestOptions(config, options),
            headers: {
                Accept: options.format === "text" ? "text/plain" : "application/octet-stream",
                ...(range === undefined ? {} : { Range: range }),
                ...options.headers,
            },
        }).pipe(
            Effect.flatMap((response) =>
                statusOk(response, "getFileContents", new Set([200, 206])),
            ),
            Effect.flatMap(
                (response): Effect.Effect<FileContents | DetailedHttpResponse, OperationError> =>
                    options.details
                        ? bodyBytes(response).pipe(
                              Effect.map((body) => ({
                                  status: response.status,
                                  statusText: response.statusText,
                                  url: response.url,
                                  headers: response.headers,
                                  body,
                              })),
                          )
                        : options.format === "text"
                          ? bodyText(response)
                          : bodyBytes(response),
            ),
        );
    };
    const putFileContents = (path: string, data: UploadData, options: PutFileContentsInput = {}) =>
        uploadBytes(data).pipe(
            Effect.flatMap((body) =>
                execute({
                    url: pathUrl(config, path),
                    method: "PUT",
                    ...requestOptions(config, options),
                    data: body,
                    headers: {
                        "Content-Type": "application/octet-stream",
                        ...(options.contentLength === false
                            ? {}
                            : {
                                  "Content-Length": `${typeof options.contentLength === "number" ? options.contentLength : body.byteLength}`,
                              }),
                        ...(options.overwrite === false ? { "If-None-Match": "*" } : {}),
                        ...options.headers,
                    },
                }),
            ),
            Effect.flatMap((response) =>
                response.status === 412 && options.overwrite === false
                    ? Effect.succeed(false)
                    : statusOk(response, "putFileContents", new Set([200, 201, 204])).pipe(
                          Effect.as(true),
                      ),
            ),
        );
    const partialUpdate = (
        path: string,
        data: UploadData,
        options: PartialUpdateInput = {},
    ): Effect.Effect<boolean, OperationError, OperationRequirements> => {
        const range = options.updateRange ?? options.range;

        // No range retains the convenience behavior of replacing the complete resource with PUT.
        if (!range) {
            return putFileContents(path, data, {
                data: options.data,
                headers: options.headers,
                signal: options.signal,
                overwrite: true,
            });
        }

        return uploadBytes(data).pipe(
            Effect.flatMap(
                (
                    uploadData,
                ): Effect.Effect<WebDavHttpResponse, OperationError, OperationRequirements> => {
                    const dataLength = uploadData.byteLength;
                    let updateRangeHeader: string;
                    let compatibilityContentRange: string | undefined;

                    if (typeof range === "string") {
                        if (range.length === 0) {
                            return Effect.fail(
                                invalid(
                                    "partialUpdateFileContents",
                                    "Update range cannot be empty",
                                ),
                            );
                        }
                        updateRangeHeader = range;
                    } else if ("append" in range) {
                        updateRangeHeader = "append";
                    } else if ("suffix" in range) {
                        if (range.suffix !== dataLength) {
                            return Effect.fail(
                                invalid(
                                    "partialUpdateFileContents",
                                    "Patch body length must equal the suffix length",
                                ),
                            );
                        }
                        updateRangeHeader = `bytes=-${range.suffix}`;
                    } else {
                        if (range.unit === "lines") {
                            return Effect.fail(
                                invalid(
                                    "partialUpdateFileContents",
                                    "Partial updates only support byte ranges",
                                ),
                            );
                        }
                        if (!Number.isSafeInteger(range.start) || range.start < 0) {
                            return Effect.fail(
                                invalid("partialUpdateFileContents", "Byte range start is invalid"),
                            );
                        }
                        if (range.end !== undefined) {
                            const expectedLength = range.end - range.start + 1;
                            if (
                                !Number.isSafeInteger(range.end) ||
                                range.end < range.start ||
                                !Number.isSafeInteger(expectedLength) ||
                                expectedLength !== dataLength
                            ) {
                                return Effect.fail(
                                    invalid(
                                        "partialUpdateFileContents",
                                        "Patch body length must equal the inclusive byte range",
                                    ),
                                );
                            }
                            updateRangeHeader = `bytes=${range.start}-${range.end}`;
                            compatibilityContentRange = `bytes ${range.start}-${range.end}/*`;
                        } else {
                            if (dataLength === 0) {
                                return Effect.fail(
                                    invalid(
                                        "partialUpdateFileContents",
                                        "Open-ended byte ranges require a non-empty body",
                                    ),
                                );
                            }
                            const end = range.start + dataLength - 1;
                            if (!Number.isSafeInteger(end)) {
                                return Effect.fail(
                                    invalid(
                                        "partialUpdateFileContents",
                                        "Byte range exceeds numeric limits",
                                    ),
                                );
                            }
                            updateRangeHeader = `bytes=${range.start}-`;
                            // Keep the legacy header for servers that implemented the pre-RFC client.
                            compatibilityContentRange = `bytes ${range.start}-${end}/*`;
                        }
                    }

                    const protocolHeaders = {
                        "Content-Type": options.contentType ?? "application/partial-update",
                        "Content-Length": `${dataLength}`,
                        "X-Update-Range": updateRangeHeader,
                        ...(compatibilityContentRange === undefined
                            ? {}
                            : { "Content-Range": compatibilityContentRange }),
                    };
                    const protocolHeaderNames = new Set(
                        Object.keys(protocolHeaders).map((name) => name.toLowerCase()),
                    );
                    const customHeaders = Object.fromEntries(
                        Object.entries(options.headers ?? {}).filter(
                            ([name]) => !protocolHeaderNames.has(name.toLowerCase()),
                        ),
                    );

                    return execute({
                        url: pathUrl(config, path),
                        method: "PATCH",
                        ...requestOptions(config, options),
                        data: uploadData,
                        headers: {
                            ...customHeaders,
                            ...protocolHeaders,
                        },
                    });
                },
            ),
            Effect.flatMap(
                (response): Effect.Effect<boolean, OperationError, OperationRequirements> => {
                    if (response.status === 409) {
                        return Effect.fail(
                            new UnsupportedFeatureError({
                                operation: "partialUpdateFileContents",
                                message:
                                    "Server does not support PATCH or partial updates (409 Conflict)",
                                status: 409,
                                url: pathUrl(config, path),
                            }),
                        );
                    }
                    return statusOk(
                        response,
                        "partialUpdateFileContents",
                        new Set([200, 204, 206]),
                    ).pipe(Effect.as(true));
                },
            ),
        );
    };
    const createReadStream = (path: string, options: CreateReadStreamInput = {}) => {
        const range =
            options.lineRange === undefined
                ? options.range === undefined
                    ? undefined
                    : options.range.unit === "lines"
                      ? formatLineRange(options.range)
                      : formatByteRange(options.range)
                : formatLineRange(options.lineRange);
        const headers =
            range === undefined ? options.headers : { Range: range, ...options.headers };
        return execute({
            url: pathUrl(config, path),
            method: "GET",
            ...requestOptions(config, options),
            headers,
        }).pipe(
            Effect.flatMap((response) =>
                statusOk(response, "createReadStream", new Set(range ? [206] : [200, 206])),
            ),
            Effect.map((response) => response.stream.pipe(Stream.mapError((cause) => cause))),
        );
    };
    const createWriteStream = (
        path: string,
        input: Stream.Stream<Uint8Array, OperationError>,
        options: CreateWriteStreamInput = {},
    ) => putFileContents(path, input, options);
    const lock = (path: string, options: LockInput = {}) =>
        DavXmlCodecLive.buildLockInfo(config.contactHref).pipe(
            Effect.flatMap((data) =>
                execute({
                    url: pathUrl(config, path),
                    method: "LOCK",
                    ...requestOptions(config, options),
                    data,
                    headers: {
                        Accept: "text/plain,application/xml",
                        Timeout: options.timeout ?? "Infinite, Second-4100000000",
                        ...(options.refreshToken ? { If: options.refreshToken } : {}),
                        ...options.headers,
                    },
                }),
            ),
            Effect.flatMap((response) => statusOk(response, "lock", new Set([200, 201, 207]))),
            Effect.flatMap(bodyText),
            Effect.flatMap((xml) => DavXmlCodecLive.parseGeneric(xml)),
            Effect.flatMap((value) => {
                const find = (value: unknown, name: string): string | undefined => {
                    if (!value || typeof value !== "object") return undefined;
                    for (const [key, child] of Object.entries(value)) {
                        if (key.split(":").pop() === name && typeof child === "string")
                            return child;
                        const nested = find(child, name);
                        if (nested) return nested;
                    }
                    return undefined;
                };
                const token = find(value, "href");
                const timeout = find(value, "timeout") ?? "";
                return token
                    ? Effect.succeed({ token, serverTimeout: timeout })
                    : Effect.fail(invalid("lock", "No lock token received"));
            }),
        );
    const unlock = (path: string, token: string, options?: OperationOptions) =>
        simple("UNLOCK", "unlock", new Set([200, 204]), path, options, { "Lock-Token": token });
    const links = {
        download: (path: string) =>
            Effect.succeed({
                url: pathUrl(config, path),
                method: "GET" as const,
                headers: {},
            }),
        upload: (path: string) =>
            Effect.succeed({
                url: pathUrl(config, path),
                method: "PUT" as const,
                headers: { "Content-Type": "application/octet-stream" },
            }),
    };
    return {
        deleteFile: {
            execute: (path: string, options?: OperationOptions) =>
                simple("DELETE", "deleteFile", new Set([200, 204, 404]), path, options),
        },
        exists: {
            execute: (path: string, options?: OperationOptions) =>
                execute({
                    url: pathUrl(config, path),
                    method: "HEAD",
                    ...requestOptions(config, options),
                }).pipe(
                    Effect.map((response) => response.status >= 200 && response.status < 400),
                    Effect.catchTag("AuthenticationError", () => Effect.succeed(false)),
                ),
        },
        moveFile: {
            execute: (from: string, to: string, options: MoveFileInput = {}) =>
                simple("MOVE", "moveFile", new Set([201, 204]), from, options, {
                    Destination: pathUrl(config, to),
                    Overwrite: options.overwrite === false ? "F" : "T",
                }),
        },
        copyFile: {
            execute: (from: string, to: string, options: CopyFileInput = {}) =>
                simple("COPY", "copyFile", new Set([201, 204]), from, options, {
                    Destination: pathUrl(config, to),
                    Overwrite: options.overwrite === false ? "F" : "T",
                    Depth: options.shallow ? "0" : "infinity",
                }),
        },
        createDirectory: {
            execute: (path: string, options: CreateDirectoryInput = {}) => {
                const directoryPath = (value: string) =>
                    value.endsWith("/") ? value : `${value}/`;
                if (!options.recursive)
                    return simple(
                        "MKCOL",
                        "createDirectory",
                        new Set([201, 405]),
                        directoryPath(path),
                        options,
                    );
                const parts = path.split("/").filter(Boolean);
                return Effect.forEach(parts, (_, index) =>
                    simple(
                        "MKCOL",
                        "createDirectory",
                        new Set([201, 405]),
                        directoryPath(parts.slice(0, index + 1).join("/")),
                        options,
                    ),
                ).pipe(Effect.asVoid);
            },
        },
        getDAVCompliance: {
            execute: (path = "", options?: OperationOptions) =>
                execute({
                    url: pathUrl(config, path),
                    method: "OPTIONS",
                    ...requestOptions(config, options),
                }).pipe(
                    Effect.flatMap((response) =>
                        statusOk(response, "getDAVCompliance", new Set([200])),
                    ),
                    Effect.map((response) =>
                        Schema.decodeUnknownSync(DAVComplianceSchema)({
                            compliance: (getHeader(response.headers, "DAV") ?? "")
                                .split(",")
                                .map((item) => item.trim())
                                .filter(Boolean),
                            server: getHeader(response.headers, "Server") ?? "",
                        }),
                    ),
                ),
        },
        getQuota: {
            execute: (options: GetQuotaInput = {}) =>
                execute({
                    url: pathUrl(config, options.path ?? ""),
                    method: "PROPFIND",
                    ...requestOptions(config, options),
                    headers: { Depth: "0", "Content-Type": "application/xml", ...options.headers },
                }).pipe(
                    Effect.flatMap((response) =>
                        statusOk(response, "getQuota", new Set([200, 207])),
                    ),
                    Effect.flatMap((response) => parse(config, response)),
                    Effect.map((result) => {
                        const prop = result.multistatus.response[0]?.propstat?.prop;
                        if (!prop) return null;
                        const used = prop["quota-used-bytes"];
                        const available = prop["quota-available-bytes"];
                        if (used === undefined || available === undefined) return null;
                        return Schema.decodeUnknownSync(DiskQuotaSchema)({
                            used: Number.isFinite(Number(used)) ? Number(used) : 0,
                            available: translateQuotaAvailable(available),
                        });
                    }),
                ),
        },
        stat,
        getDirectoryContents: getContents,
        search: {
            execute: (path: string, options?: SearchInput) => {
                const base = requestOptions(config, options);
                const headers: Headers = {
                    Accept: "text/plain,application/xml",
                    "Content-Type": "application/xml; charset=utf-8",
                    ...base.headers,
                };
                return execute({
                    url: pathUrl(config, path),
                    method: "SEARCH",
                    ...base,
                    headers,
                }).pipe(
                    Effect.flatMap((response) => statusOk(response, "search", new Set([200, 207]))),
                    Effect.flatMap((response) => parse(config, response)),
                    Effect.map((result) =>
                        Schema.decodeUnknownSync(SearchResultSchema)({
                            truncated: result.multistatus.response.some(
                                (item) => statusCode(item.status ?? item.propstat?.status) === 507,
                            ),
                            results: result.multistatus.response
                                .filter((item) => item.propstat !== undefined)
                                .map((item) => fileStat(item.href, item.propstat?.prop)),
                        }),
                    ),
                );
            },
        },
        getFileContents: { execute: getFileContents },
        putFileContents: { execute: putFileContents },
        partialUpdateFileContents: { execute: partialUpdate },
        createReadStream: { execute: createReadStream },
        createWriteStream: { execute: createWriteStream },
        lock: { execute: lock },
        unlock: { execute: unlock },
        fileLinks: links,
        customRequest: {
            execute: (request: {
                url: string;
                method: string;
                headers?: Headers;
                data?: unknown;
            }) =>
                execute({
                    ...request,
                    method: request.method as never,
                    data: request.data as never,
                    ...requestOptions(config, request),
                }).pipe(
                    Effect.flatMap((response) =>
                        bodyText(response).pipe(
                            Effect.map((body) => ({
                                status: response.status,
                                statusText: response.statusText,
                                url: response.url,
                                headers: response.headers,
                                body,
                            })),
                        ),
                    ),
                ),
        },
    };
};

const globMatch = (pattern: string, value: string): boolean => {
    const escaped = pattern
        .replace(/[.+^${}()|[\]\\]/g, "\\$&")
        .replace(/\*/g, ".*")
        .replace(/\?/g, ".");
    return new RegExp(`^${escaped}$`).test(value);
};

export interface OperationsLayerOptions {
    readonly config: NormalizedWebDavConfig;
}
export const OperationsLive = (options: OperationsLayerOptions) => {
    const operations = makeLive(options.config);
    return Layer.mergeAll(
        Layer.succeed(DeleteFile, operations.deleteFile),
        Layer.succeed(Exists, operations.exists),
        Layer.succeed(MoveFile, operations.moveFile),
        Layer.succeed(CopyFile, operations.copyFile),
        Layer.succeed(CreateDirectory, operations.createDirectory),
        Layer.succeed(GetDavCompliance, operations.getDAVCompliance),
        Layer.succeed(GetQuota, operations.getQuota),
        Layer.succeed(Stat, { execute: operations.stat } as Stat),
        Layer.succeed(GetDirectoryContents, {
            execute: operations.getDirectoryContents,
        } as GetDirectoryContents),
        Layer.succeed(Search, operations.search),
        Layer.succeed(CustomRequest, operations.customRequest),
        Layer.succeed(GetFileContents, operations.getFileContents),
        Layer.succeed(PutFileContents, operations.putFileContents),
        Layer.succeed(PartialUpdateFileContents, operations.partialUpdateFileContents),
        Layer.succeed(CreateReadStream, operations.createReadStream),
        Layer.succeed(CreateWriteStream, operations.createWriteStream),
        Layer.succeed(Lock, operations.lock),
        Layer.succeed(Unlock, operations.unlock),
        Layer.succeed(FileLinks, operations.fileLinks),
    );
};

export const OperationsLayer = OperationsLive;
export const OperationsTest = OperationsLive;