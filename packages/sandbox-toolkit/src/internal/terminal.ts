import { Data, Option, Schema } from "effect";

import type { Status } from "../generated/Status.ts";
import type { TerminalSize } from "../generated/TerminalSize.ts";

/** Discriminants are wire identifiers, see `Process.md`; reordering changes the format. */
const STDIN = 0;

const STDOUT = 1;

const EXIT = 3;

const RESIZE = 4;

/**
 * One WebSocket message, which is itself a whole frame: the first byte selects
 * the channel and the rest is the payload.
 */
export type Frame = Data.TaggedEnum<{
  Stdin: { readonly data: Uint8Array };
  Stdout: { readonly data: Uint8Array };
  Exit: { readonly status: Status };
  Resize: { readonly size: TerminalSize };
}>;

export const Frame = Data.taggedEnum<Frame>();

const encoder = new TextEncoder();

const statusSchema = Schema.fromJsonString(
  Schema.Union([
    Schema.Struct({ status: Schema.Literal("exited"), exit_code: Schema.Number }),
    Schema.Struct({ status: Schema.Literal("failed"), message: Schema.String }),
  ]),
);

const resizeBytes = (size: TerminalSize): Uint8Array =>
  new Uint8Array([size.rows >> 8, size.rows & 0xff, size.cols >> 8, size.cols & 0xff]);

const join = (channel: number, payload: Uint8Array): Uint8Array => {
  const message = new Uint8Array(1 + payload.length);
  message[0] = channel;
  message.set(payload, 1);

  return message;
};

export const encodeFrame = Frame.$match({
  Stdin: ({ data }) => join(STDIN, data),
  Stdout: ({ data }) => join(STDOUT, data),
  Exit: ({ status }) => join(EXIT, encoder.encode(JSON.stringify(status))),
  Resize: ({ size }) => join(RESIZE, resizeBytes(size)),
});

const parseStatus = (payload: Uint8Array): Status | undefined =>
  Option.getOrUndefined(
    Schema.decodeUnknownOption(statusSchema)(new TextDecoder().decode(payload)),
  );

/** Rejects an empty message, an unknown channel, or a payload that violates its channel. */
export const decodeFrame = (message: Uint8Array): Option.Option<Frame> => {
  const payload = message.subarray(1);

  switch (message[0]) {
    case STDIN:
      return Option.some(Frame.Stdin({ data: payload }));
    case STDOUT:
      return Option.some(Frame.Stdout({ data: payload }));
    case RESIZE: {
      const [rowsHi, rowsLo, colsHi, colsLo] = payload;

      return rowsHi === undefined ||
        rowsLo === undefined ||
        colsHi === undefined ||
        colsLo === undefined
        ? Option.none()
        : Option.some(
            Frame.Resize({
              size: { rows: (rowsHi << 8) | rowsLo, cols: (colsHi << 8) | colsLo },
            }),
          );
    }

    case EXIT: {
      const status = parseStatus(payload);

      return status === undefined ? Option.none() : Option.some(Frame.Exit({ status }));
    }

    default:
      return Option.none();
  }
};

const HTTP_PREFIX = /^http/;

/**
 * The WebSocket URL for a server-relative attach path. The base URL is the same
 * one the HTTP client was configured with, only its scheme switches to `ws`.
 */
export const webSocketUrl = (baseUrl: string, path: string): string => {
  const root = baseUrl.replace(/\/+$/, "");
  const joined = path.startsWith("/") ? `${root}${path}` : `${root}/${path}`;

  return joined.replace(HTTP_PREFIX, "ws");
};
