import { Effect, Layer } from "effect";
import {
  HttpClient,
  HttpClientError,
  HttpClientRequest,
  HttpClientResponse,
} from "effect/unstable/http";
import { expect, test } from "vite-plus/test";

import * as Client from "../src/internal/client.ts";
import * as Workspace from "../src/Workspace.ts";

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

const bodyOf = (request: HttpClientRequest.HttpClientRequest): unknown => {
  const body = request.body;

  return body._tag === "Uint8Array" ? JSON.parse(new TextDecoder().decode(body.body)) : undefined;
};

const errorBody = (code: string, message: string, status = 404) =>
  Response.json({ error: { code, message, request_id: "req-1" } }, { status });

const layerOf = (handle: Handler) =>
  Workspace.layer.pipe(
    Layer.provide(
      Client.layer({ baseUrl }).pipe(
        Layer.provide(Layer.succeed(HttpClient.HttpClient, stub(handle))),
      ),
    ),
  );

const run = <A, E>(
  program: Effect.Effect<A, E, Workspace.WorkspaceService>,
  handle: Handler,
): Promise<A> => Effect.runPromise(Effect.provide(program, layerOf(handle)));

const handle = (id: string, access = "read-write") => ({ id, properties: { access } });

const created = (id: string, access = "read-write") =>
  Response.json(handle(id, access), { status: 201 });

test("registers a workspace and returns its handle", async () => {
  const requests: Array<{ url: string; body: unknown }> = [];

  const program = Effect.flatMap(Workspace.Workspace, (workspace) =>
    workspace.create({ id: "docs", root: "/srv/docs" }),
  );

  const value = await run(program, (request, url) => {
    requests.push({ url: url.toString(), body: bodyOf(request) });

    return created("docs");
  });

  expect(value).toEqual(handle("docs"));
  expect(requests).toEqual([
    {
      url: "http://sandbox.test/workspaces",
      body: {
        id: "docs",
        root: "/srv/docs",
        properties: { access: "read-write" },
      },
    },
  ]);
});

test("sends the requested access mode", async () => {
  let body: unknown;

  const program = Effect.flatMap(Workspace.Workspace, (workspace) =>
    workspace.create({ id: "sealed", root: "/srv/sealed", access: "read-only" }),
  );

  const value = await run(program, (request) => {
    body = bodyOf(request);

    return created("sealed", "read-only");
  });

  expect(body).toEqual({
    id: "sealed",
    root: "/srv/sealed",
    properties: { access: "read-only" },
  });
  expect(value).toEqual(handle("sealed", "read-only"));
});

test("rejects a malformed id before sending a request", async () => {
  let requested = false;

  const error = await run(
    Effect.flatMap(Workspace.Workspace, (workspace) =>
      Effect.flip(workspace.create({ id: "Bad", root: "/srv/docs" })),
    ),
    () => {
      requested = true;

      return created("docs");
    },
  );

  expect(error).toBeInstanceOf(Workspace.InvalidWorkspaceId);
  expect(error).toMatchObject({ workspace: "Bad" });
  expect(requested).toBe(false);
});

test("maps a duplicate id to WorkspaceExists", async () => {
  const error = await run(
    Effect.flatMap(Workspace.Workspace, (workspace) =>
      Effect.flip(workspace.create({ id: "docs", root: "/srv/docs" })),
    ),
    () => errorBody("conflict", "workspace `docs` already exists", 409),
  );

  expect(error).toBeInstanceOf(Workspace.WorkspaceExists);
  expect(error).toMatchObject({ workspace: "docs" });
});

test("lists the registered workspaces", async () => {
  let seen: string | undefined;

  const program = Effect.flatMap(Workspace.Workspace, (workspace) => workspace.list());

  const value = await run(program, (_request, url) => {
    seen = url.toString();

    return Response.json({ workspaces: [handle("alpha"), handle("zeta", "read-only")] });
  });

  expect(seen).toBe("http://sandbox.test/workspaces");
  expect(value).toEqual([handle("alpha"), handle("zeta", "read-only")]);
});

test("fetches one workspace by id", async () => {
  let seen: string | undefined;

  const program = Effect.flatMap(Workspace.Workspace, (workspace) => workspace.get("docs"));

  const value = await run(program, (_request, url) => {
    seen = url.toString();

    return Response.json(handle("docs"));
  });

  expect(seen).toBe("http://sandbox.test/workspaces/docs");
  expect(value).toEqual(handle("docs"));
});

test("maps an unknown workspace to WorkspaceNotFound", async () => {
  const error = await run(
    Effect.flatMap(Workspace.Workspace, (workspace) => Effect.flip(workspace.get("missing"))),
    () => errorBody("not_found", "workspace `missing`"),
  );

  expect(error).toBeInstanceOf(Workspace.WorkspaceNotFound);
  expect(error).toMatchObject({ workspace: "missing" });
});

test("unregisters a workspace", async () => {
  let seen: { method: string; url: string } | undefined;

  const program = Effect.flatMap(Workspace.Workspace, (workspace) => workspace.remove("docs"));

  await run(program, (request, url) => {
    seen = { method: request.method, url: url.toString() };

    return new Response(null, { status: 204 });
  });

  expect(seen).toEqual({ method: "DELETE", url: "http://sandbox.test/workspaces/docs" });
});

test("maps the refusal reasons of a removal distinctly", async () => {
  const remove: Effect.Effect<Workspace.WorkspaceError, void, Workspace.WorkspaceService> =
    Effect.flatMap(Workspace.Workspace, (workspace) => Effect.flip(workspace.remove("docs")));

  const notFound = await run(remove, () => errorBody("not_found", "gone"));
  const inUse = await run(remove, () => errorBody("workspace_in_use", "in use", 409));
  const managed = await run(remove, () => errorBody("managed_workspace", "managed", 403));

  expect(notFound).toBeInstanceOf(Workspace.WorkspaceNotFound);
  expect(inUse).toBeInstanceOf(Workspace.WorkspaceInUse);
  expect(managed).toBeInstanceOf(Workspace.ManagedWorkspace);
});

test("rejects a malformed id when fetching and removing", async () => {
  const errors = await run(
    Effect.flatMap(Workspace.Workspace, (workspace) =>
      Effect.all([Effect.flip(workspace.get("Bad")), Effect.flip(workspace.remove("Bad"))]),
    ),
    () => errorBody("not_found", "unused"),
  );

  for (const error of errors) {
    expect(error).toBeInstanceOf(Workspace.InvalidWorkspaceId);
  }
});

test("passes a transport failure through", async () => {
  const error = await run(
    Effect.flatMap(Workspace.Workspace, (workspace) => Effect.flip(workspace.list())),
    () => new Error("connection refused"),
  );

  expect(error).toBeInstanceOf(Client.TransportError);
});

test("createScoped unregisters the workspace when the scope closes", async () => {
  const requests: Array<string> = [];

  const program = Effect.flatMap(Workspace.Workspace, (workspace) =>
    workspace.createScoped({ id: "docs", root: "/srv/docs" }),
  );

  const value = await run(Effect.scoped(program), (request, url) => {
    requests.push(`${request.method} ${url.toString()}`);

    return request.method === "DELETE" ? new Response(null, { status: 204 }) : created("docs");
  });

  expect(value).toEqual(handle("docs"));
  expect(requests).toEqual([
    "POST http://sandbox.test/workspaces",
    "DELETE http://sandbox.test/workspaces/docs",
  ]);
});

test("createScoped tolerates a workspace that is already gone", async () => {
  const program = Effect.flatMap(Workspace.Workspace, (workspace) =>
    workspace.createScoped({ id: "docs", root: "/srv/docs" }),
  );

  const value = await run(Effect.scoped(program), (request) =>
    request.method === "DELETE" ? errorBody("not_found", "workspace `docs`", 404) : created("docs"),
  );

  expect(value).toEqual(handle("docs"));
});

test("createScoped does not release a workspace it never acquired", async () => {
  const requests: Array<string> = [];

  const program = Effect.flatMap(Workspace.Workspace, (workspace) =>
    Effect.scoped(Effect.flip(workspace.createScoped({ id: "docs", root: "/srv/docs" }))),
  );

  const error = await run(program, (request, url) => {
    requests.push(`${request.method} ${url.toString()}`);

    return errorBody("conflict", "workspace `docs` already exists", 409);
  });

  expect(error).toBeInstanceOf(Workspace.WorkspaceExists);
  expect(requests).toEqual(["POST http://sandbox.test/workspaces"]);
});
