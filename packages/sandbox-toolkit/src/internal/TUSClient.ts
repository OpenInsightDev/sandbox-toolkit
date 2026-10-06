import type { Readable } from "node:stream";

import { Cause, Context, Data, Effect, Layer, Option, Predicate, Queue, Stream } from "effect";
import {
  DetailedError,
  Upload as TusUpload,
  type UploadInput,
  type UploadOptions as TusUploadOptions,
} from "tus-js-client";

import { Client, layer as clientLayer } from "./client.ts";

/** An upload that could not be created, transferred, or terminated. */
export class UploadError extends Data.TaggedError("UploadError")<{
  readonly message: string;
  /** HTTP status of the request that failed, when the failure came from a response. */
  readonly status: number | undefined;
  /** Upload URL the failure belongs to, when one had been assigned already. */
  readonly uploadUrl: string | undefined;
  readonly cause: unknown;
}> {}

/**
 * The bytes to upload: a local path, an in-memory buffer, a Node or web
 * readable stream, or an explicit path range.
 */
export type UploadSource =
  | string
  | Uint8Array
  | Readable
  | ReadableStream<Uint8Array>
  | {
      readonly path: string;
      readonly start?: number | undefined;
      readonly end?: number | undefined;
    };

export interface UploadOptions {
  readonly source: UploadSource;
  /**
   * Metadata attached to the creation request, carried by tus' `Upload-Metadata`
   * header. Values must be strings; the server decodes them from base64.
   */
  readonly metadata?: Readonly<Record<string, string>> | undefined;
  /** Headers added to every request of the upload. */
  readonly headers?: Readonly<Record<string, string>> | undefined;
  /**
   * Maximum size of a `PATCH` body, in bytes. Required for stream sources;
   * leave unset to send the whole source in one request.
   */
  readonly chunkSize?: number | undefined;
  /**
   * Delays before each retry, in milliseconds. The array length is the retry
   * budget; an empty array disables retries.
   */
  readonly retryDelays?: ReadonlyArray<number> | undefined;
  /**
   * Send the body in the creation request (`creation-with-upload`), trading a
   * round trip for the requirement that the server supports the extension.
   */
  readonly uploadDataDuringCreation?: boolean | undefined;
  /** Upload the source as a stream of unknown length (`Upload-Defer-Length`). */
  readonly uploadLengthDeferred?: boolean | undefined;
  /** Number of parts uploaded in parallel; requires the server's concatenation extension. */
  readonly parallelUploads?: number | undefined;
  /**
   * Resume from a previously created upload instead of creating a new one.
   * The value is the `url` of an earlier {@link UploadResult} or
   * {@link UploadEvent.UrlAvailable}.
   */
  readonly uploadUrl?: string | undefined;
  /**
   * Upload endpoint overriding the one derived from the client base URL and the
   * server's mount point.
   */
  readonly endpoint?: string | undefined;
}

/** A finished upload, addressed by its server-side URL. */
export interface UploadResult {
  readonly url: string;
  /** Status of the response that completed the upload. */
  readonly status: number;
}

/**
 * One notification from a running upload, in order: any number of `Progress`
 * and one `UrlAvailable`, then exactly one `Success`.
 */
export type UploadEvent = Data.TaggedEnum<{
  Progress: { readonly bytesSent: number; readonly bytesTotal: number | null };
  UrlAvailable: { readonly url: string };
  Success: { readonly result: UploadResult };
}>;

export const UploadEvent = Data.taggedEnum<UploadEvent>();

export interface TerminateOptions {
  readonly headers?: Readonly<Record<string, string>> | undefined;
  readonly retryDelays?: ReadonlyArray<number> | undefined;
}

export interface TUSClient {
  /**
   * Upload a source and stream its progress, ending with `Success`.
   *
   * **Details**
   *
   * The request starts when the stream is pulled and the created resource is
   * kept if the stream is interrupted, so a later call can resume it by passing
   * the URL reached through {@link UploadEvent.UrlAvailable} as `uploadUrl`.
   * Use {@link terminate} to delete a resource that should not be resumed.
   */
  readonly upload: (options: UploadOptions) => Stream.Stream<UploadEvent, UploadError>;

  /**
   * Upload a source and return its result, discarding progress notifications.
   */
  readonly uploadResult: (options: UploadOptions) => Effect.Effect<UploadResult, UploadError>;

  /** Delete an upload resource created by a previous call (termination extension). */
  readonly terminate: (url: string, options?: TerminateOptions) => Effect.Effect<void, UploadError>;
}

export const TUSClient: Context.Service<TUSClient, TUSClient> = Context.Service("tus-client");

const toUploadInput = (source: UploadSource): UploadInput =>
  Predicate.isString(source) ? { path: source } : source;

const toUploadError = (cause: unknown, uploadUrl: string | null | undefined): UploadError => {
  const response = cause instanceof DetailedError ? cause.originalResponse : undefined;

  return new UploadError({
    message: cause instanceof Error ? cause.message : String(cause),
    status: response?.getStatus(),
    uploadUrl: uploadUrl ?? undefined,
    cause,
  });
};

const absoluteUrl = (value: string): Effect.Effect<string, UploadError> =>
  Effect.try({
    try: () => new URL(value).toString(),
    catch: () =>
      new UploadError({
        message: `upload endpoint must be an absolute URL: ${value}`,
        status: undefined,
        uploadUrl: undefined,
        cause: undefined,
      }),
  });

/** The server's one upload mount point, which no workspace prefix changes. */
const UPLOAD_PATH = "/tus";

export const make = Effect.fn("TUSClient.make")(function* () {
  const client = yield* Client;

  const endpoint = (override: string | undefined): Effect.Effect<string, UploadError> =>
    absoluteUrl(override ?? `${client.baseUrl}${UPLOAD_PATH}`);

  const upload = ((uploadOptions: UploadOptions) =>
    Stream.unwrap(
      endpoint(uploadOptions.endpoint).pipe(
        Effect.map((resolved) =>
          Stream.callback<UploadEvent, UploadError>((queue) =>
            Effect.gen(function* () {
              // tus merges options as `{ ...defaults, ...provided }`, so an
              // explicit `undefined` would erase a default rather than fall back
              // to it. Only the values the caller set are forwarded.
              const tusOptions: Partial<TusUploadOptions> = {
                endpoint: resolved,
                metadata: { ...uploadOptions.metadata },
                headers: { ...uploadOptions.headers },
                onProgress: (bytesSent, bytesTotal) => {
                  Queue.offerUnsafe(queue, UploadEvent.Progress({ bytesSent, bytesTotal }));
                },
                onUploadUrlAvailable: () => {
                  if (upload.url !== null) {
                    Queue.offerUnsafe(queue, UploadEvent.UrlAvailable({ url: upload.url }));
                  }
                },
                onSuccess: ({ lastResponse }) => {
                  Queue.offerUnsafe(
                    queue,
                    UploadEvent.Success({
                      result: { url: upload.url ?? "", status: lastResponse.getStatus() },
                    }),
                  );
                  Queue.endUnsafe(queue);
                },
                onError: (error) => {
                  Queue.failCauseUnsafe(queue, Cause.fail(toUploadError(error, upload.url)));
                },
              };

              if (uploadOptions.chunkSize !== undefined) {
                tusOptions.chunkSize = uploadOptions.chunkSize;
              }

              if (uploadOptions.retryDelays !== undefined) {
                tusOptions.retryDelays = [...uploadOptions.retryDelays];
              }

              if (uploadOptions.uploadLengthDeferred !== undefined) {
                tusOptions.uploadLengthDeferred = uploadOptions.uploadLengthDeferred;
              }

              if (uploadOptions.uploadDataDuringCreation !== undefined) {
                tusOptions.uploadDataDuringCreation = uploadOptions.uploadDataDuringCreation;
              }

              if (uploadOptions.parallelUploads !== undefined) {
                tusOptions.parallelUploads = uploadOptions.parallelUploads;
              }

              if (uploadOptions.uploadUrl !== undefined) {
                tusOptions.uploadUrl = uploadOptions.uploadUrl;
              }

              const upload = new TusUpload(toUploadInput(uploadOptions.source), tusOptions);

              yield* Effect.acquireRelease(
                Effect.sync(() => upload.start()),
                // Interrupting the stream only aborts the transfer; the resource
                // survives so the caller can resume it.
                () => Effect.promise(() => upload.abort()),
              );
            }),
          ),
        ),
      ),
    )) satisfies TUSClient["upload"];

  const uploadResult = ((uploadOptions: UploadOptions) =>
    Stream.runFold(
      upload(uploadOptions),
      () => Option.none<UploadResult>(),
      (acc, event) => (Predicate.isTagged("Success")(event) ? Option.some(event.result) : acc),
    ).pipe(
      Effect.flatMap(
        Option.match({
          onNone: () => Effect.die("TUSClient: upload ended without a result"),
          onSome: Effect.succeed,
        }),
      ),
    )) satisfies TUSClient["uploadResult"];

  const terminate = ((url: string, terminateOptions?: TerminateOptions) => {
    const tusOptions: Partial<TusUploadOptions> = { headers: { ...terminateOptions?.headers } };

    if (terminateOptions?.retryDelays !== undefined) {
      tusOptions.retryDelays = [...terminateOptions.retryDelays];
    }

    return Effect.tryPromise({
      try: () => TusUpload.terminate(url, tusOptions),
      catch: (cause) => toUploadError(cause, url),
    });
  }) satisfies TUSClient["terminate"];

  return TUSClient.of({ upload, uploadResult, terminate });
});

/** The tus service over the server's upload mount point. */
export const layer = (config: { readonly baseUrl?: string | URL | undefined } = {}) =>
  Layer.effect(TUSClient, make()).pipe(Layer.provide(clientLayer(config)));
