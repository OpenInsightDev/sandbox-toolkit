import { Effect, Schema } from "effect";
import {
    DAVProperties,
    EntityDecoderOptions,
    Headers,
    HttpMethod,
    Path,
    ProgressEvent,
    Range,
    RequestData,
    Url,
} from "#/domain.ts";
import { InvalidConfigError } from "#/error.ts";

export const AuthType = Schema.Literals(["auto", "digest", "none", "password", "token"] as const);
export type AuthType = Schema.Schema.Type<typeof AuthType>;

export const OAuthToken = Schema.Struct({
    access_token: Schema.String,
    token_type: Schema.String,
    refresh_token: Schema.optional(Schema.String),
});
export type OAuthToken = Schema.Schema.Type<typeof OAuthToken>;

export const WebDavConfig = Schema.Struct({
    remoteUrl: Url,
    remoteBasePath: Schema.optional(Path),
    authType: Schema.optional(AuthType),
    username: Schema.optional(Schema.String),
    password: Schema.optional(Schema.String),
    token: Schema.optional(OAuthToken),
    ha1: Schema.optional(Schema.String),
    headers: Schema.optional(Headers),
    withCredentials: Schema.optional(Schema.Boolean),
    contactHref: Schema.optional(Url),
    entityDecoder: Schema.optional(EntityDecoderOptions),
});
export type WebDavConfig = Schema.Schema.Type<typeof WebDavConfig>;

export const NormalizedWebDavConfig = Schema.Struct({
    remoteUrl: Url,
    remoteBasePath: Schema.optional(Path),
    authType: AuthType,
    username: Schema.optional(Schema.String),
    password: Schema.optional(Schema.String),
    token: Schema.optional(OAuthToken),
    ha1: Schema.optional(Schema.String),
    headers: Headers,
    withCredentials: Schema.Boolean,
    contactHref: Url,
    entityDecoder: Schema.optional(EntityDecoderOptions),
});
export type NormalizedWebDavConfig = Schema.Schema.Type<typeof NormalizedWebDavConfig>;

const methodOptionFields = {
    data: Schema.optional(RequestData),
    headers: Schema.optional(Headers),
    signal: Schema.optional(Schema.instanceOf(globalThis.AbortSignal)),
} as const;

export const MethodOptions = Schema.Struct(methodOptionFields);
export type MethodOptions = Schema.Schema.Type<typeof MethodOptions>;

export const StatOptions = Schema.Struct({
    ...methodOptionFields,
    details: Schema.optional(Schema.Boolean),
});
export const SearchOptions = StatOptions;
export const GetQuotaOptions = Schema.Struct({
    ...methodOptionFields,
    details: Schema.optional(Schema.Boolean),
    path: Schema.optional(Path),
});
export const GetDirectoryContentsOptions = Schema.Struct({
    ...methodOptionFields,
    deep: Schema.optional(Schema.Boolean),
    glob: Schema.optional(Schema.String),
    includeSelf: Schema.optional(Schema.Boolean),
    details: Schema.optional(Schema.Boolean),
});
export const GetFileContentsOptions = Schema.Struct({
    ...methodOptionFields,
    details: Schema.optional(Schema.Boolean),
    format: Schema.optional(Schema.Literals(["binary", "text"] as const)),
});
export const CreateDirectoryOptions = Schema.Struct({
    ...methodOptionFields,
    recursive: Schema.optional(Schema.Boolean),
});
export const CreateReadStreamOptions = Schema.Struct({
    ...methodOptionFields,
    range: Schema.optional(Range),
});
export const CreateWriteStreamOptions = Schema.Struct({
    ...methodOptionFields,
    overwrite: Schema.optional(Schema.Boolean),
});
export const MoveFileOptions = Schema.Struct({
    ...methodOptionFields,
    overwrite: Schema.optional(Schema.Boolean),
});
export const CopyFileOptions = Schema.Struct({
    ...methodOptionFields,
    overwrite: Schema.optional(Schema.Boolean),
    shallow: Schema.optional(Schema.Boolean),
});
export const LockOptions = Schema.Struct({
    ...methodOptionFields,
    refreshToken: Schema.optional(Schema.String),
    timeout: Schema.optional(Schema.String),
});
export const PutFileContentsOptions = Schema.Struct({
    ...methodOptionFields,
    contentLength: Schema.optional(Schema.Union([Schema.Boolean, Schema.Number])),
    overwrite: Schema.optional(Schema.Boolean),
});
export const PartialUpdateOptions = Schema.Struct({
    ...methodOptionFields,
    range: Schema.optional(Range),
    contentType: Schema.optional(Schema.String),
});

export const RequestOptions = Schema.Struct({
    url: Url,
    method: HttpMethod,
    data: Schema.optional(RequestData),
    headers: Schema.optional(Headers),
    withCredentials: Schema.optional(Schema.Boolean),
    signal: Schema.optional(Schema.instanceOf(globalThis.AbortSignal)),
});
export const ResponseMetadata = Schema.Struct({
    status: Schema.Int,
    statusText: Schema.String,
    url: Url,
    headers: Headers,
});

export const defaultContactHref =
    "https://github.com/perry-mitchell/webdav-client/blob/master/LOCK_CONTACT.md";

const invalidConfig = (message: string, cause?: string): InvalidConfigError =>
    new InvalidConfigError({
        operation: "config",
        message,
        ...(cause === undefined ? {} : { cause }),
    });

const validateConfig = (
    config: WebDavConfig,
): Effect.Effect<NormalizedWebDavConfig, InvalidConfigError> => {
    const authType = config.authType ?? (config.username || config.password ? "password" : "none");
    const hasPasswordCredentials = config.username !== undefined || config.password !== undefined;

    if (authType === "password" && (!config.username || !config.password)) {
        return Effect.fail(invalidConfig("password authentication requires username and password"));
    }
    if (authType === "digest" && (!config.username || (!config.password && !config.ha1))) {
        return Effect.fail(
            invalidConfig("digest authentication requires username and password or ha1"),
        );
    }
    if (authType === "token" && config.token === undefined) {
        return Effect.fail(invalidConfig("token authentication requires token"));
    }
    if (authType === "none" && hasPasswordCredentials) {
        return Effect.fail(
            invalidConfig("none authentication cannot include username or password"),
        );
    }
    if (authType === "auto" && !config.username && !config.password) {
        return Effect.fail(invalidConfig("auto authentication requires username or password"));
    }

    return Effect.succeed({
        remoteUrl: config.remoteUrl,
        ...(config.remoteBasePath === undefined ? {} : { remoteBasePath: config.remoteBasePath }),
        authType,
        ...(config.username === undefined ? {} : { username: config.username }),
        ...(config.password === undefined ? {} : { password: config.password }),
        ...(config.token === undefined ? {} : { token: config.token }),
        ...(config.ha1 === undefined ? {} : { ha1: config.ha1 }),
        headers: config.headers ?? {},
        withCredentials: config.withCredentials ?? false,
        contactHref: config.contactHref ?? defaultContactHref,
        ...(config.entityDecoder === undefined ? {} : { entityDecoder: config.entityDecoder }),
    });
};

export const decodeWebDavConfig = (
    input: unknown,
): Effect.Effect<NormalizedWebDavConfig, InvalidConfigError> =>
    Schema.decodeUnknownEffect(WebDavConfig)(input).pipe(
        Effect.mapError((error) =>
            invalidConfig("configuration does not match the WebDAV schema", String(error)),
        ),
        Effect.flatMap(validateConfig),
    );

export const decodeWebDavConfigSync = (input: unknown): NormalizedWebDavConfig => {
    try {
        const config = Schema.decodeUnknownSync(WebDavConfig)(input);
        return Effect.runSync(validateConfig(config));
    } catch (error) {
        if (error instanceof InvalidConfigError) {
            throw error;
        }
        throw invalidConfig("configuration does not match the WebDAV schema", String(error));
    }
};

export type ConfigPath = Schema.Schema.Type<typeof Path>;
export type ConfigHeaders = Schema.Schema.Type<typeof Headers>;
export type ConfigProgressEvent = Schema.Schema.Type<typeof ProgressEvent>;
export type ConfigDAVProperties = Schema.Schema.Type<typeof DAVProperties>;