import { Effect, Layer, Stream } from "effect";
import {
  HttpClient,
  HttpClientError,
  HttpClientRequest,
  HttpClientResponse,
} from "effect/unstable/http";
import { expect, test } from "vite-plus/test";

import { ApiError, Client, TransportError, layer } from "../src/internal/client.ts";

const baseUrl = "http://sandbox.test";

type Handler = (request: HttpClientRequest.HttpClientRequest, url: URL) => Response | Error;

const stub = (handle: Handler): HttpClient.HttpClient =>
  HttpClient.make((request, url) =>
    Effect.suspend(() => {
      const result = handle(request, url);

      return result instanceof Error
        ? Effect.fail(
            new HttpClientError.HttpClientError({
              reason: new HttpClientError.TransportError({ request, cause: result }),
            }),
          )
        : Effect.succeed(HttpClientResponse.fromWeb(request, result));
    }),
  );

const clientLayer = (handle: Handler) =>
  layer({ baseUrl }).pipe(Layer.provide(Layer.succeed(HttpClient.HttpClient, stub(handle))));

const run = <A, E>(program: Effect.Effect<A, E, Client>, handle: Handler): Promise<A> =>
  Effect.runPromise(Effect.provide(program, clientLayer(handle)));

const errorBody = (code: string, message: string, requestId = "req-1") =>
  Response.json({ error: { code, message, request_id: requestId } }, { status: 404 });

test("prepends the base URL and decodes JSON responses", async () => {
  let seen: string | undefined;

  const program = Effect.gen(function* () {
    const client = yield* Client;

    return yield* client.json<{ id: string }>(HttpClientRequest.get("workspaces/docs"));
  });

  const value = await run(program, (_request, url) => {
    seen = url.toString();

    return Response.json({ id: "docs" });
  });

  expect(seen).toBe("http://sandbox.test/workspaces/docs");
  expect(value).toEqual({ id: "docs" });
});

test("maps the error envelope onto ApiError", async () => {
  const program = Effect.gen(function* () {
    const client = yield* Client;

    return yield* client.json(HttpClientRequest.get("workspaces/missing"));
  });

  await expect(
    run(program, () => errorBody("not_found", "no such workspace")),
  ).rejects.toMatchObject({
    _tag: "ApiError",
    status: 404,
    code: "not_found",
    requestId: "req-1",
  });
});

test("falls back to a status-derived code when the body is not an envelope", async () => {
  const program = Effect.gen(function* () {
    const client = yield* Client;

    return yield* client.json(HttpClientRequest.get("workspaces/docs"));
  });

  await expect(run(program, () => new Response("not json", { status: 500 }))).rejects.toMatchObject(
    { _tag: "ApiError", status: 500, code: "http_500" },
  );
});

test("reports transport failures as TransportError", async () => {
  const program = Effect.gen(function* () {
    const client = yield* Client;

    return yield* client.text(HttpClientRequest.get("ping"));
  });

  await expect(run(program, () => new Error("connection refused"))).rejects.toBeInstanceOf(
    TransportError,
  );
});

test("ignores the body of a no-content response", async () => {
  const program = Effect.gen(function* () {
    const client = yield* Client;

    return yield* client.void(HttpClientRequest.delete("workspaces/docs"));
  });

  await expect(run(program, () => new Response(null, { status: 204 }))).resolves.toBeUndefined();
});

test("streams response bytes on success", async () => {
  const program = Effect.gen(function* () {
    const client = yield* Client;

    return yield* Stream.runFold(
      client.stream(HttpClientRequest.get("workspaces/docs/fs/notes.txt")),
      () => "",
      (text, chunk) => text + new TextDecoder().decode(chunk),
    );
  });

  const value = await run(program, () => new Response("hello"));

  expect(value).toBe("hello");
});

test("fails the stream before emitting when the status is not 2xx", async () => {
  const program = Effect.gen(function* () {
    const client = yield* Client;

    return yield* Stream.runCollect(client.stream(HttpClientRequest.get("workspaces/missing")));
  });

  await expect(
    run(program, () => errorBody("not_found", "no such workspace")),
  ).rejects.toBeInstanceOf(ApiError);
});
