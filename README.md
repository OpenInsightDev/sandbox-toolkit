# Sandbox Toolkit

One HTTP API that gives programs and AI agents safe access to a directory: read and write files, run commands, and open interactive terminals — with the same operations served as MCP tools.

## Install

```bash
cargo install --path crates/sandbox-toolkit
```

The build fetches pinned tool binaries and embeds them into the server: `fd`, `rg`, `curl`, `uv`/`uvx` and `deno`. They materialize on startup, so anything the server spawns can call them by name with no host install and no network.

## Usage

```bash
sbxtkt --root ./data
```

Register a directory as a workspace, then operate inside it:

```bash
curl -sS -X POST http://127.0.0.1:3000/workspaces \
  -H 'content-type: application/json' \
  -d '{"id":"docs","root":"/abs/path/to/data"}'

curl -sS -X QUERY 'http://127.0.0.1:3000/workspaces/docs/fs?type=content' \
  -H 'content-type: application/json' \
  -d '{"path":"notes.md"}'

curl -sS -X PUT 'http://127.0.0.1:3000/workspaces/docs/fs?type=content' \
  -H 'content-type: application/json' \
  -d '{"path":"notes.md","content":"hello\n"}'
```

Everything under `/workspaces/{id}` is addressed by workspace-relative paths: files (`/fs`), commands (`/exec`), terminals (`/pty`), and skills (`/skills`). Direct mode drops the prefix and uses absolute paths instead. MCP clients connect to `/mcp`.

A workspace is the enforced boundary: every path is canonicalized and must resolve inside the root, and a `read-only` workspace rejects mutations.

## Features

- **Files** — read text and raw bytes, stream, list and glob; write, create directories and symlinks, patch metadata, truncate, delete. Strong `ETag`s on responses.
- **Commands** — `exec` runs an executable or a shell script with a caller-supplied `cwd`, `env` and argv, returning JSON for fast commands and a multiplexed frame stream for long-running ones.
- **Terminals** — interactive PTY sessions over WebSocket (HTTP/2 extended CONNECT, RFC 8441).
- **Skills** — [Agent Skills](https://agentskills.io/specification) discovered from `.agents/skills`, with progressive disclosure and an auto-derived read-only workspace for bundled files.
- **Uploads** — resumable uploads at `/tus`, a reverse proxy to a bundled [tusd](https://github.com/tus/tusd) the server runs as its sidecar.
- **One model, two surfaces** — each operation is implemented once against a shared Rust model that generates the HTTP wire format, the MCP schemas, and the TypeScript SDK types.

## TypeScript SDK

Effect-based services (`Workspace`, `FileSystem`, `Mcp`, `Plugin`, `Process`, `Skill`, `Terminal`) with typed errors and streaming. A registered workspace carries the services bound to its own mount prefix, so `docs.process` runs against `/workspaces/docs/exec` without any further wiring.

```ts
import { Effect, Layer } from "effect";
import { FetchHttpClient } from "effect/unstable/http";

import * as Workspace from "./src/Workspace.ts";

const program = Effect.gen(function* () {
  const workspaces = yield* Workspace.Workspace;
  const docs = yield* workspaces.create({ id: "docs", root: "/srv/project" });

  return yield* docs.process.$`rg --files`;
});

await Effect.runPromise(
  Effect.provide(
    program,
    Workspace.layer({ baseUrl: "http://127.0.0.1:3000" }).pipe(Layer.provide(FetchHttpClient.layer)),
  ),
);
```

Each service also has a `layer` of its own for its global mount point, where paths and mount points are not workspace-relative.

## Configuration

| Flag | Environment variable | Description | Default |
| --- | --- | --- | --- |
| `--root` | `SBXTKT_ROOT` | Base directory the server treats as its working root | `.` |
| `--host` | `SBXTKT_HOST` | Bind address | `127.0.0.1` |
| `--port`, `-p` | `SBXTKT_PORT` | Bind port | `3000` |
| — | `RUST_LOG` | Log filter (e.g. `debug`) | `info,sbxtkt=debug` |

## Development

The repository uses [Vite+](https://viteplus.dev/guide/) for the unified toolchain, driving the Rust workspace too.

```bash
vp install           # install dependencies
vp run check         # cargo clippy, format, lint and type-check
vp run test          # unit tests, then the package tests
vp run rust:e2e      # the end-to-end suite, in its Linux container
vp run rust:build    # build the server binary the end-to-end tests spawn
```

The end-to-end suite drives the server and the processes it spawns, so it runs
inside a Linux container: `docker` on CI, Apple's `container` on macOS, and
`SBXTKT_TEST_RUNTIME` picks one where neither is right. The image is
`rust:1.97-bookworm`, overridable with `SBXTKT_TEST_IMAGE`. Its build caches live
in `${XDG_CACHE_HOME:-$HOME/.cache}/sandbox-toolkit`, overridable with
`SBXTKT_TEST_CACHE`, and every worktree shares them, so only the first run pays
for a full compile. `cargo test` runs the suite by hand, and needs the same
container.

With [`sccache`](https://github.com/mozilla/sccache) on `PATH`, every checkout
compiles into one shared cache, so a new git worktree no longer pays for the
dependency graph again; without it the build falls back to plain `rustc`.

```bash
brew install sccache   # or: cargo binstall sccache
```

`SCCACHE_DIR` moves the cache off the boot volume and `SCCACHE_CACHE_SIZE`
(default 10 GiB) caps it.

The PTY crate keeps its own tests (`cargo test -p pty`).

## License

Released under the MIT License.
