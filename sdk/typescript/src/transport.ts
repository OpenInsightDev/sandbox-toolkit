import { Context, Effect, Layer, Schema, Stream } from "effect";
import {
    HttpBody,
    HttpClient,
    HttpClientError,
    HttpClientRequest,
    HttpClientResponse,
    HttpMethod,
} from "effect/unstable/http";
import { Headers, HttpMethod as HttpMethodSchema, RequestData, Url } from "#/domain.ts";
import { AbortError, TransportError } from "#/error.ts";

export const WebDavHttpRequestSchema = Schema.Struct({
    url: Url,
    method: HttpMethodSchema,
    headers: Schema.optional(Headers),
    data: Schema.optional(RequestData),
    signal: Schema.optional(Schema.instanceOf(globalThis.AbortSignal)),
    withCredentials: Schema.optional(Schema.Boolean),
});

export interface WebDavHttpRequest {
    readonly url: string;
    readonly method: string;
    readonly headers?: Headers;
    readonly data?: RequestData;
    readonly signal?: AbortSignal;
    readonly withCredentials?: boolean;
}

export const WebDavHttpResponseMetadata = Schema.Struct({
    status: Schema.Int,
    statusText: Schema.String,
    url: Url,
    headers: Headers,
});

export interface WebDavHttpResponse {
    readonly status: number;
    readonly statusText: string;
    readonly url: string;
    readonly headers: Headers;
    readonly stream: Stream.Stream<Uint8Array, TransportError | AbortError>;
}

export interface Transport {
    readonly execute: (
        request: WebDavHttpRequest,
    ) => Effect.Effect<WebDavHttpResponse, TransportError | AbortError>;
}

export const Transport = Context.Service<Transport>("webdav/Transport");

const operationName = (request: WebDavHttpRequest): string =>
    `HTTP ${request.method.toUpperCase()}`;

const errorMessage = (cause: unknown): string =>
    cause instanceof Error ? cause.message : String(cause);

const makeTransportError = (request: WebDavHttpRequest, cause: unknown): TransportError =>
    new TransportError({
        operation: operationName(request),
        message: `HTTP transport failed: ${errorMessage(cause)}`,
        cause: errorMessage(cause),
        url: request.url,
    });

const makeAbortError = (request: WebDavHttpRequest, cause?: unknown): AbortError =>
    new AbortError({
        operation: operationName(request),
        message: "HTTP request was aborted",
        ...(cause === undefined ? {} : { cause: errorMessage(cause) }),
        url: request.url,
    });

const isAbortCause = (cause: unknown): boolean => {
    if (cause instanceof Error && cause.name === "AbortError") {
        return true;
    }
    if (typeof cause !== "object" || cause === null) {
        return false;
    }
    if ("_tag" in cause && cause._tag === "AbortError") {
        return true;
    }
    if ("cause" in cause && isAbortCause(cause.cause)) {
        return true;
    }
    return "reason" in cause && isAbortCause(cause.reason);
};

const toHttpBody = (
    data: RequestData | undefined,
): string | Uint8Array | ArrayBuffer | undefined => {
    if (data === undefined) {
        return undefined;
    }
    if (typeof data === "string") {
        return data;
    }
    if (data instanceof Uint8Array) {
        return data;
    }
    if (data instanceof ArrayBuffer) {
        return data;
    }
    return JSON.stringify(data);
};

const toRequest = (request: WebDavHttpRequest): HttpClientRequest.HttpClientRequest => {
    const base = HttpClientRequest.make(request.method.toUpperCase() as HttpMethod.HttpMethod)(
        request.url,
        {
            headers: request.headers,
        },
    );
    if (request.data === undefined) {
        return base;
    }
    const body =
        request.data instanceof Uint8Array
            ? HttpBody.uint8Array(request.data)
            : request.data instanceof ArrayBuffer
              ? HttpBody.uint8Array(new Uint8Array(request.data))
              : typeof request.data === "string"
                ? HttpBody.text(request.data)
                : HttpBody.jsonUnsafe(request.data);
    return HttpClientRequest.setBody(base, body);
};

const toHeaders = (headers: Readonly<Record<string, string>>): Headers => ({ ...headers });

type ResponseDetails = {
    readonly statusText: string;
    readonly url: string;
    readonly cleanup: () => void;
};

const responseDetails = new WeakMap<HttpClientRequest.HttpClientRequest, ResponseDetails>();

const mapHttpClientError = (
    request: WebDavHttpRequest,
    cause: HttpClientError.HttpClientError,
): TransportError | AbortError => {
    if (cause.reason._tag === "TransportError" && isAbortCause(cause.reason.cause)) {
        return makeAbortError(request, cause.reason.cause);
    }
    return isAbortCause(cause)
        ? makeAbortError(request, cause)
        : makeTransportError(request, cause);
};

const executeFetch = (
    request: WebDavHttpRequest,
): Effect.Effect<WebDavHttpResponse, TransportError | AbortError> => {
    const client = HttpClient.make((effectRequest, url, signal) =>
        Effect.tryPromise({
            try: (effectSignal) => {
                if (request.signal?.aborted) {
                    return Promise.reject(makeAbortError(request));
                }

                const controller = new AbortController();
                let cleanedUp = false;
                const abort = () => controller.abort();
                const cleanup = () => {
                    if (cleanedUp) {
                        return;
                    }
                    cleanedUp = true;
                    request.signal?.removeEventListener("abort", abort);
                    signal.removeEventListener("abort", abort);
                    effectSignal.removeEventListener("abort", abort);
                };
                request.signal?.addEventListener("abort", abort, { once: true });
                signal.addEventListener("abort", abort, { once: true });
                effectSignal.addEventListener("abort", abort, { once: true });

                return globalThis
                    .fetch(url, {
                        method: effectRequest.method,
                        headers: effectRequest.headers,
                        body: toHttpBody(request.data),
                        credentials: request.withCredentials ? "include" : "same-origin",
                        signal: controller.signal,
                    })
                    .then((rawResponse) => {
                        const response = HttpClientResponse.fromWeb(effectRequest, rawResponse);
                        responseDetails.set(response.request, {
                            statusText: rawResponse.statusText,
                            url: rawResponse.url || request.url,
                            cleanup: () => {
                                controller.abort();
                                cleanup();
                            },
                        });
                        return response;
                    })
                    .catch((cause) => {
                        cleanup();
                        throw cause;
                    });
            },
            catch: (cause) =>
                new HttpClientError.HttpClientError({
                    reason: new HttpClientError.TransportError({
                        request: toRequest(request),
                        cause,
                    }),
                }),
        }),
    );

    return client.execute(toRequest(request)).pipe(
        Effect.map((response) => {
            const details = responseDetails.get(response.request);
            const cleanup = details?.cleanup ?? (() => undefined);
            const stream = response.stream.pipe(
                Stream.mapError((cause) =>
                    isAbortCause(cause)
                        ? makeAbortError(request, cause)
                        : makeTransportError(request, cause),
                ),
                Stream.ensuring(Effect.sync(cleanup)),
            );
            return {
                status: response.status,
                statusText: details?.statusText ?? "",
                url: details?.url ?? response.request.url,
                headers: toHeaders(response.headers),
                stream,
            };
        }),
        Effect.mapError((cause) => mapHttpClientError(request, cause)),
    );
};

const fetchTransport: Transport = {
    execute: executeFetch,
};

export const FetchHttpTransport = {
    layer: Layer.succeed(Transport, fetchTransport),
    nodeLayer: Layer.succeed(Transport, fetchTransport),
    browserLayer: Layer.succeed(Transport, fetchTransport),
};

export interface TestTransportOptions {
    readonly execute: (
        request: WebDavHttpRequest,
    ) => Effect.Effect<WebDavHttpResponse, TransportError | AbortError>;
}

export const TestTransport = (options: TestTransportOptions): Layer.Layer<Transport> =>
    Layer.succeed(Transport, options);

export const responseFromBytes = (options: {
    readonly status?: number;
    readonly statusText?: string;
    readonly url: string;
    readonly headers?: Headers;
    readonly body?: Uint8Array;
}): WebDavHttpResponse => ({
    status: options.status ?? 200,
    statusText: options.statusText ?? "",
    url: options.url,
    headers: options.headers ?? {},
    stream: Stream.succeed(options.body ?? new Uint8Array()),
});

export const responseFromText = (options: {
    readonly status?: number;
    readonly statusText?: string;
    readonly url: string;
    readonly headers?: Headers;
    readonly body?: string;
}): WebDavHttpResponse =>
    responseFromBytes({
        ...options,
        body: new TextEncoder().encode(options.body ?? ""),
    });