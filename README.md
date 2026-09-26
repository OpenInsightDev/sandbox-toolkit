# Sandbox Toolkit

One HTTP API that gives programs and AI agents safe access to a directory: read and write files, run commands, and open interactive terminals — with the same operations served as MCP tools.

## Install

```bash
cargo install --path crates/sandbox-toolkit --features binaries
```

The build fetches pinned tool binaries and embeds them into the server. `fd` and `rg` are always included; `jaq`, `jq`, `uv`/`uvx` and `deno` come with the `binaries` feature. They materialize on startup, so anything the server spawns can call them by name with no host install and no network.

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

curl -sS -X PUT 'http://127.0.0.1:3000/workspaces/docs/fs?type=file' \
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
- **One model, two surfaces** — each operation is implemented once against a shared Rust model that generates the HTTP wire format, the MCP schemas, and the TypeScript SDK types.

## TypeScript SDK

Effect-based services (`Workspace`, `FileSystem`, `Process`, `Skill`, `Terminal`) with typed errors and streaming.

```ts
import { Effect, Layer } from "effect";

import * as FileSystem from "./src/FileSystem.ts";
import { layerFetch } from "./src/internal/client.ts";

const workspace = { id: "docs", properties: { access: "read-write" } } as const;

const program = Effect.gen(function* () {
  const fs = yield* FileSystem.FileSystem;
  return yield* fs.readFileString("notes.md");
});

await Effect.runPromise(
  Effect.provide(
    program,
    FileSystem.layerForWorkspace({ workspace }).pipe(
      Layer.provide(layerFetch({ baseUrl: "http://127.0.0.1:3000" })),
    ),
  ),
);
```

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
vp check             # format, lint and type-check
vp test              # cargo test --workspace, then the package tests
vp run rust:build    # build the server binary the integration tests spawn
```

The PTY crate keeps its own tests (`cargo test -p pty`). See [`ONBOARD.md`](./ONBOARD.md) for known work items.

## License

Released under the MIT License.
