import { Schema } from "effect";

/** A parsed DAV property value. Unknown extension properties keep their raw shape. */
export const DAVPropertyValue = Schema.Unknown;

export const DAVProperties = Schema.StructWithRest(
    Schema.Struct({
        displayname: Schema.optional(Schema.String),
        resourcetype: Schema.optional(Schema.Record(Schema.String, Schema.Unknown)),
        getlastmodified: Schema.optional(Schema.String),
        getetag: Schema.optional(Schema.String),
        getcontentlength: Schema.optional(Schema.Union([Schema.String, Schema.Number])),
        getcontenttype: Schema.optional(Schema.String),
        "quota-available-bytes": Schema.optional(Schema.Union([Schema.String, Schema.Number])),
        "quota-used-bytes": Schema.optional(Schema.Union([Schema.String, Schema.Number])),
    }),
    [Schema.Record(Schema.String, Schema.Unknown)],
);

export const DAVPropStat = Schema.Struct({
    prop: DAVProperties,
    status: Schema.String,
    responsedescription: Schema.optional(Schema.String),
});

export const DAVResultResponse = Schema.Struct({
    href: Schema.String,
    propstat: Schema.optional(DAVPropStat),
    status: Schema.optional(Schema.String),
    responsedescription: Schema.optional(Schema.String),
});

export const DAVResult = Schema.Struct({
    multistatus: Schema.Struct({
        response: Schema.Array(DAVResultResponse),
    }),
});

export const FileType = Schema.Literals(["file", "directory"] as const);

export const FileStat = Schema.Struct({
    filename: Schema.String,
    basename: Schema.String,
    lastmod: Schema.String,
    size: Schema.Number,
    type: FileType,
    etag: Schema.NullOr(Schema.String),
    mime: Schema.optional(Schema.String),
    props: Schema.optional(DAVProperties),
});

export const DiskQuotaAvailable = Schema.Union([
    Schema.Literals(["unknown", "unlimited"] as const),
    Schema.Number,
]);

export const DiskQuota = Schema.Struct({
    used: Schema.Number,
    available: DiskQuotaAvailable,
});

export const SearchResult = Schema.Struct({
    truncated: Schema.Boolean,
    results: Schema.Array(FileStat),
});

export const DAVCompliance = Schema.Struct({
    compliance: Schema.Array(Schema.String),
    server: Schema.String,
});

export const LockResponse = Schema.Struct({
    serverTimeout: Schema.String,
    token: Schema.String,
});

export const Headers = Schema.Record(Schema.String, Schema.String);

export const RequestData = Schema.Union([
    Schema.String,
    Schema.Uint8Array,
    Schema.instanceOf(globalThis.ArrayBuffer),
    Schema.Record(Schema.String, Schema.Unknown),
]);

export const HttpMethod = Schema.String.pipe(
    Schema.check(Schema.makeFilter((method) => method.length > 0 || "HTTP method cannot be empty")),
);

export const Url = Schema.String.pipe(
    Schema.check(
        Schema.makeFilter((value) => {
            try {
                const url = new URL(value);
                return (
                    url.protocol === "http:" ||
                    url.protocol === "https:" ||
                    "URL must use http or https"
                );
            } catch {
                return "URL must be absolute and valid";
            }
        }),
    ),
);

export const Path = Schema.String.pipe(
    Schema.check(
        Schema.makeFilter((path) => !path.includes("\u0000") || "Path cannot contain a null byte"),
    ),
);

export const Range = Schema.Struct({
    start: Schema.Int.pipe(
        Schema.check(
            Schema.makeFilter((value) => value >= 0 || "Range start must be non-negative"),
        ),
    ),
    end: Schema.optional(
        Schema.Int.pipe(
            Schema.check(
                Schema.makeFilter((value) => value >= 0 || "Range end must be non-negative"),
            ),
        ),
    ),
}).pipe(
    Schema.check(
        Schema.makeFilter(
            (range) =>
                range.end === undefined ||
                range.end >= range.start ||
                "Range end must be greater than or equal to start",
        ),
    ),
);

export const ResponseDataDetailed = <A extends Schema.Schema<any>>(data: A) =>
    Schema.Struct({
        data,
        headers: Headers,
        status: Schema.Int.pipe(
            Schema.check(
                Schema.makeFilter((value) => value >= 100 || "Status must be valid HTTP status"),
            ),
        ),
        statusText: Schema.String,
        url: Url,
    });

export const ProgressEvent = Schema.Struct({
    loaded: Schema.Number,
    total: Schema.Number,
});

export const EntityDecoderOptions = Schema.Struct({
    limit: Schema.optional(
        Schema.Struct({
            maxTotalExpansions: Schema.optional(Schema.Int),
            maxExpandedLength: Schema.optional(Schema.Int),
        }),
    ),
});

export type DAVProperties = Schema.Schema.Type<typeof DAVProperties>;
export type DAVPropStat = Schema.Schema.Type<typeof DAVPropStat>;
export type DAVResultResponse = Schema.Schema.Type<typeof DAVResultResponse>;
export type DAVResult = Schema.Schema.Type<typeof DAVResult>;
export type FileType = Schema.Schema.Type<typeof FileType>;
export type FileStat = Schema.Schema.Type<typeof FileStat>;
export type DiskQuotaAvailable = Schema.Schema.Type<typeof DiskQuotaAvailable>;
export type DiskQuota = Schema.Schema.Type<typeof DiskQuota>;
export type SearchResult = Schema.Schema.Type<typeof SearchResult>;
export type DAVCompliance = Schema.Schema.Type<typeof DAVCompliance>;
export type LockResponse = Schema.Schema.Type<typeof LockResponse>;
export type Headers = Schema.Schema.Type<typeof Headers>;
export type RequestData = Schema.Schema.Type<typeof RequestData>;
export type Range = Schema.Schema.Type<typeof Range>;
export type ProgressEvent = Schema.Schema.Type<typeof ProgressEvent>;
export type EntityDecoderOptions = Schema.Schema.Type<typeof EntityDecoderOptions>;