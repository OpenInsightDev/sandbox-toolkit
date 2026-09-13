import { Schema } from "effect";
import { Url } from "#/domain.ts";

const CommonErrorFields = {
    operation: Schema.String,
    message: Schema.String,
    cause: Schema.optional(Schema.String),
    url: Schema.optional(Url),
    status: Schema.optional(Schema.Int),
    requestId: Schema.optional(Schema.String),
} as const;

export class ConfigError extends Schema.TaggedError<ConfigError>()(
    "ConfigError",
    CommonErrorFields,
) {}
export class InvalidConfigError extends Schema.TaggedError<InvalidConfigError>()(
    "InvalidConfigError",
    CommonErrorFields,
) {}
export class InvalidPathError extends Schema.TaggedError<InvalidPathError>()(
    "InvalidPathError",
    CommonErrorFields,
) {}
export class InvalidRangeError extends Schema.TaggedError<InvalidRangeError>()(
    "InvalidRangeError",
    CommonErrorFields,
) {}
export class UnsupportedFeatureError extends Schema.TaggedError<UnsupportedFeatureError>()(
    "UnsupportedFeatureError",
    CommonErrorFields,
) {}
export class TransportError extends Schema.TaggedError<TransportError>()(
    "TransportError",
    CommonErrorFields,
) {}
export class HttpStatusError extends Schema.TaggedError<HttpStatusError>()(
    "HttpStatusError",
    CommonErrorFields,
) {}
export class AuthenticationError extends Schema.TaggedError<AuthenticationError>()(
    "AuthenticationError",
    CommonErrorFields,
) {}
export class DigestChallengeError extends Schema.TaggedError<DigestChallengeError>()(
    "DigestChallengeError",
    CommonErrorFields,
) {}
export class XmlParseError extends Schema.TaggedError<XmlParseError>()(
    "XmlParseError",
    CommonErrorFields,
) {}
export class XmlEncodeError extends Schema.TaggedError<XmlEncodeError>()(
    "XmlEncodeError",
    CommonErrorFields,
) {}
export class InvalidDavResponseError extends Schema.TaggedError<InvalidDavResponseError>()(
    "InvalidDavResponseError",
    CommonErrorFields,
) {}
export class ResponseDecodeError extends Schema.TaggedError<ResponseDecodeError>()(
    "ResponseDecodeError",
    CommonErrorFields,
) {}
export class ContentLengthError extends Schema.TaggedError<ContentLengthError>()(
    "ContentLengthError",
    CommonErrorFields,
) {}
export class StreamError extends Schema.TaggedError<StreamError>()(
    "StreamError",
    CommonErrorFields,
) {}
export class AbortError extends Schema.TaggedError<AbortError>()("AbortError", CommonErrorFields) {}

export const WebDavError = Schema.Union([
    ConfigError,
    InvalidConfigError,
    InvalidPathError,
    InvalidRangeError,
    UnsupportedFeatureError,
    TransportError,
    HttpStatusError,
    AuthenticationError,
    DigestChallengeError,
    XmlParseError,
    XmlEncodeError,
    InvalidDavResponseError,
    ResponseDecodeError,
    ContentLengthError,
    StreamError,
    AbortError,
]);

export type WebDavError = Schema.Schema.Type<typeof WebDavError>;
export type WebDavErrorTag = WebDavError extends { _tag: infer Tag } ? Tag : never;

export const isWebDavError = (value: unknown): value is WebDavError =>
    Schema.is(WebDavError)(value);

export const errorFields = CommonErrorFields;