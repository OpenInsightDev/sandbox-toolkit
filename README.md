# Sandbox Toolkit

[Design](./docs/design/Toolkit.md) • [File API](./docs/design/FileSystem.md) • [Process](./docs/design/Process.md) • [MCP](./docs/design/MCP.md) • [Skill](./docs/design/Skill.md)

A sandbox server that gives agents and programs safe access to a remote directory. Register a folder, then read and write files, run commands, and reach your MCP servers — all through one HTTP API, with the same operations exposed as MCP tools.

## Features

- **Workspaces as boundaries** — register any directory under an id; every operation stays inside it, and nothing that escapes the root gets through.
- **Full file API** — browse, read, write, move, copy, delete, stat and list, with conditional writes that catch concurrent edits instead of silently overwriting them.
- **Commands and scripts** — `exec` runs a program directly, `shell` runs a script, and both stream output back with the final exit code.
- **Real terminals** — interactive pty sessions over WebSocket for REPLs, editors, and anything else that needs a TTY.
- **MCP in one place** — register local subprocess or remote URL servers, then reach every one of them through a single `/mcp` endpoint.
- **Agent Skills** — discover skills from `.agents/skills`, and serve their metadata, content, and bundled files to a remote agent.
- **Tools included** — `fd`, `rg`, and `jaq` (plus `uv`, `uvx`, `deno` when built in) are callable by name inside the sandbox, with no pre-install and no network.

## Installation

```bash
cargo install --path crates/sandbox-toolkit
# or run straight from the source tree
cargo run -- --root ./data
```

## Quick Start

```bash
# 1. expose ./data on port 3000
cargo run -- --root ./data

# 2. register it as a workspace
curl -X POST http://127.0.0.1:3000/workspaces \
  -H 'content-type: application/json' \
  -d '{"id": "docs", "root": "/abs/path/to/data"}'

# 3. read a file back, relative to the workspace root
curl http://127.0.0.1:3000/workspaces/docs/fs/notes.md
```

Every other capability hangs off the workspace prefix you just registered. Point an MCP client at the same server for the tool interface:

```
http://127.0.0.1:3000/mcp
```

## Documentation

| Document                                   | Description                                             |
| ------------------------------------------ | ------------------------------------------------------- |
| [Toolkit](./docs/design/Toolkit.md)        | The two calling surfaces and the shared parameter model |
| [Workspace](./docs/design/Workspace.md)    | Registration, addressing, properties, lifecycle         |
| [File System](./docs/design/FileSystem.md) | File API, path model, ETag conditions, error protocol   |
| [Process](./docs/design/Process.md)        | `exec`, `shell`, and pty sessions                       |
| [MCP](./docs/design/MCP.md)                | Registering and proxying MCP servers                    |
| [Skill](./docs/design/Skill.md)            | Discovering and serving Agent Skills                    |
| [Binary](./docs/design/Binary.md)          | Bundled command-line tools                              |

## Configuration

| Flag     | Environment Variable | Description                         | Default          |
| -------- | -------------------- | ----------------------------------- | ---------------- |
| `--root` | `SBXKIT_ROOT`        | Root directory the server may touch | `.`              |
| `--host` | `SBXKIT_HOST`        | Bind address                        | `127.0.0.1`      |
| `--port` | `SBXKIT_PORT`        | Bind port                           | `3000`           |
| —        | `RUST_LOG`           | Log filter (e.g. `debug`)           | `info,sbxkit=debug` |

## License

MIT License.
