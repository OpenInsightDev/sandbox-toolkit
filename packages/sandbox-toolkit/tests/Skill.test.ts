import { Effect, Layer } from "effect";
import {
  HttpClient,
  HttpClientError,
  HttpClientRequest,
  HttpClientResponse,
} from "effect/unstable/http";
import { expect, test } from "vite-plus/test";

import * as Client from "../src/internal/client.ts";
import * as Skill from "../src/Skill.ts";
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

const errorBody = (code: string, message: string, status = 404) =>
  Response.json({ error: { code, message, request_id: "req-1" } }, { status });

const workspaceHandle = (id: string): Workspace.WorkspaceHandle => ({
  id,
  properties: { access: "read-write" },
});

const layerOf = (workspace: string | undefined, handle: Handler) => {
  const client = Client.layer({ baseUrl }).pipe(
    Layer.provide(Layer.succeed(HttpClient.HttpClient, stub(handle))),
  );

  const skill =
    workspace === undefined
      ? Skill.layer
      : Skill.layerForWorkspace({ workspace: workspaceHandle(workspace) });

  return skill.pipe(Layer.provide(client));
};

const run = <A, E>(
  program: Effect.Effect<A, E, Skill.Skill>,
  workspace: string | undefined,
  handle: Handler,
): Promise<A> => Effect.runPromise(Effect.provide(program, layerOf(workspace, handle)));

const listSkills = Effect.flatMap(Skill.Skill, (skill) => skill.list());

const readSkill = (skillId: string) => Effect.flatMap(Skill.Skill, (skill) => skill.read(skillId));

test("lists the skills at a workspace mount and maps the derived fields", async () => {
  let seen: string | undefined;

  const value = await run(listSkills, "docs", (_request, url) => {
    seen = url.toString();

    return Response.json({
      skills: [
        {
          id: "deploy",
          root: "/home/u/.agents/skills/deploy",
          name: "deploy",
          description: "Roll out a service.",
          license: "MIT",
          compatibility: "Requires kubectl",
          metadata: { author: "acme" },
          uri: "/workspaces/docs/skills/deploy",
          workspace_id: "skill-docs-deploy",
        },
        {
          id: "other",
          root: "/home/u/.agents/skills/other",
          name: "other",
          description: "Another skill.",
          uri: "/workspaces/docs/skills/other",
          workspace_id: "skill-docs-other",
        },
      ],
    });
  });

  expect(seen).toBe("http://sandbox.test/workspaces/docs/skills");
  expect(value).toEqual({
    skills: [
      {
        id: "deploy",
        root: "/home/u/.agents/skills/deploy",
        name: "deploy",
        description: "Roll out a service.",
        license: "MIT",
        compatibility: "Requires kubectl",
        metadata: { author: "acme" },
        uri: "/workspaces/docs/skills/deploy",
        workspace: "skill-docs-deploy",
      },
      {
        id: "other",
        root: "/home/u/.agents/skills/other",
        name: "other",
        description: "Another skill.",
        uri: "/workspaces/docs/skills/other",
        workspace: "skill-docs-other",
      },
    ],
  });
});

test("lists the global mount in direct mode", async () => {
  let seen: string | undefined;

  const value = await run(listSkills, undefined, (_request, url) => {
    seen = url.toString();

    return Response.json({
      skills: [
        {
          id: "deploy",
          root: "/home/u/.agents/skills/deploy",
          name: "deploy",
          description: "Roll out a service.",
          uri: "/skills/deploy",
          workspace_id: "skill-deploy",
        },
      ],
    });
  });

  expect(seen).toBe("http://sandbox.test/skills");
  expect(value.skills[0]?.uri).toBe("/skills/deploy");
  expect(value.skills[0]?.workspace).toBe("skill-deploy");
});

test("sends only the paging options that were given", async () => {
  const seen: Array<string> = [];

  await run(
    Effect.flatMap(Skill.Skill, (skill) => skill.list({ offset: 1, limit: 2 })),
    "docs",
    (_request, url) => {
      seen.push(url.toString());

      return Response.json({ skills: [] });
    },
  );

  await run(listSkills, "docs", (_request, url) => {
    seen.push(url.toString());

    return Response.json({ skills: [] });
  });

  expect(seen).toEqual([
    "http://sandbox.test/workspaces/docs/skills?offset=1&limit=2",
    "http://sandbox.test/workspaces/docs/skills",
  ]);
});

test("reads a body without the frontmatter as markdown", async () => {
  let seen: string | undefined;

  const value = await run(readSkill("deploy"), "docs", (_request, url) => {
    seen = url.toString();

    return new Response("# Deploy\n\nShip it.", {
      headers: { "content-type": "text/markdown; charset=utf-8" },
    });
  });

  expect(seen).toBe("http://sandbox.test/workspaces/docs/skills/deploy");
  expect(value).toBe("# Deploy\n\nShip it.");
});

test("rejects a malformed id before sending a request", async () => {
  let requested = false;

  const error = await run(
    Effect.flatMap(Skill.Skill, (skill) => Effect.flip(skill.read("Bad"))),
    "docs",
    () => {
      requested = true;

      return Response.json({ skills: [] });
    },
  );

  expect(error).toBeInstanceOf(Skill.InvalidSkillId);
  expect(error).toMatchObject({ skillId: "Bad" });
  expect(requested).toBe(false);
});

test("rejects a repeated or leading hyphen", async () => {
  const program = Effect.flatMap(Skill.Skill, (skill) =>
    Effect.forEach(["a--b", "-bad", "bad-"], (skillId) => Effect.flip(skill.read(skillId))),
  );

  const errors = await run(program, "docs", () => Response.json({ skills: [] }));

  for (const error of errors) {
    expect(error).toBeInstanceOf(Skill.InvalidSkillId);
  }
});

test("maps an unknown skill to SkillNotFound", async () => {
  const error = await run(
    Effect.flatMap(Skill.Skill, (skill) => Effect.flip(skill.read("missing"))),
    "docs",
    () => errorBody("not_found", "skill `missing` does not exist"),
  );

  expect(error).toBeInstanceOf(Skill.SkillNotFound);
  expect(error).toMatchObject({ skillId: "missing" });
});

test("passes other server errors through as ApiError", async () => {
  const error = await run(
    Effect.flatMap(Skill.Skill, (skill) => Effect.flip(skill.read("deploy"))),
    "docs",
    () => errorBody("bad_request", "invalid", 400),
  );

  expect(error).toBeInstanceOf(Client.ApiError);
  expect(error).toMatchObject({ code: "bad_request" });
});

test("propagates a list failure as ApiError", async () => {
  const error = await run(
    Effect.flatMap(Skill.Skill, (skill) => Effect.flip(skill.list())),
    "docs",
    () => errorBody("not_found", "workspace `docs` does not exist"),
  );

  expect(error).toBeInstanceOf(Client.ApiError);
  expect(error).toMatchObject({ code: "not_found" });
});
