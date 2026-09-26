import type { Dispatcher } from "@effect/platform-node/Undici";
import { Agent, H2CClient, WebSocket } from "@effect/platform-node/Undici";
import { Effect, Layer, Predicate } from "effect";
import * as Socket from "effect/unstable/socket/Socket";

const isProtocols = (
  options: Socket.WebSocketConstructorOptions | undefined,
): options is string | Array<string> => Predicate.isString(options) || Array.isArray(options);

/**
 * Dials WebSockets with HTTP/2 extended CONNECT (RFC 8441) through undici,
 * instead of the HTTP/1.1 `Upgrade` handshake performed by the `ws` package and
 * the global `WebSocket`.
 *
 * undici only reaches its extended-CONNECT path when the underlying dispatcher
 * negotiates HTTP/2, so cleartext `ws:` endpoints use an h2c client and `wss:`
 * endpoints an agent whose ALPN prefers `h2`. The opening handshake is then sent
 * as `:method = CONNECT` with `:protocol = websocket`, which a server supporting
 * RFC 8441 accepts without upgrading the connection.
 */
export const layer: Layer.Layer<Socket.WebSocketConstructor> = Layer.effect(
  Socket.WebSocketConstructor,
  Effect.gen(function* () {
    // H2CClient is pinned to one origin, so cache one per cleartext origin and
    // tear them down with the layer's scope.
    const h2c = yield* Effect.acquireRelease(
      Effect.sync(() => new Map<string, Dispatcher>()),
      (cache) =>
        Effect.forEach(cache.values(), (dispatcher) => Effect.promise(() => dispatcher.destroy()), {
          discard: true,
        }),
    );

    const tls = yield* Effect.acquireRelease(
      Effect.sync(() => new Agent({ connect: { allowH2: true, preferH2: true } })),
      (agent) => Effect.promise(() => agent.destroy()),
    );

    const dispatcherFor = (protocol: string, origin: string): Dispatcher => {
      if (protocol === "wss:") return tls;

      const httpOrigin = `http://${origin.slice("ws://".length)}`;
      const existing = h2c.get(httpOrigin);

      if (existing !== undefined) return existing;

      const created = new H2CClient(httpOrigin);
      h2c.set(httpOrigin, created);

      return created;
    };

    return (url, options) => {
      const parsed = new URL(url);
      const protocols = isProtocols(options) ? options : undefined;

      const headers =
        options !== undefined && !isProtocols(options) && options.headers !== undefined
          ? { ...options.headers }
          : undefined;

      // SAFETY: undici's WebSocket implements the structural `WebSocketLike`
      // surface (`readyState`, add/removeEventListener, `close`, `send`) that
      // `Socket` consumes; only the platform-specific event payload types differ.
      return new WebSocket(url, {
        dispatcher: dispatcherFor(parsed.protocol, parsed.origin),
        headers,
        protocols,
      }) as Socket.WebSocketLike;
    };
  }),
);
