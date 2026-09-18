import { Schema } from "effect";

/** The request parameters were rejected (HTTP 400). */
export class InvalidRequestError extends Schema.TaggedError<InvalidRequestError>()(
  "InvalidRequestError",
  { message: Schema.String },
) {}

/** The requested resource does not exist (HTTP 404). */
export class NotFoundError extends Schema.TaggedError<NotFoundError>()("NotFoundError", {
  message: Schema.String,
}) {}

/** The server failed to handle a valid request (any other non-2xx status). */
export class ServerError extends Schema.TaggedError<ServerError>()("ServerError", {
  status: Schema.Int,
  message: Schema.String,
}) {}

/** No decodable response: network failure, bad body, or unexpected shape. */
export class TransportError extends Schema.TaggedError<TransportError>()("TransportError", {
  message: Schema.String,
  cause: Schema.Defect(),
}) {}

/**
 * Every failure the SDK can produce. Handle a reason with
 * `Effect.catchReason("SandboxToolkitError", "NotFoundError", ...)`.
 */
export class SandboxToolkitError extends Schema.TaggedError<SandboxToolkitError>()(
  "SandboxToolkitError",
  {
    reason: Schema.Union([InvalidRequestError, NotFoundError, ServerError, TransportError]),
  },
) {}
