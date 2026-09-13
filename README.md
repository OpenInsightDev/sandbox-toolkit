<div align="center">
  <h1>Sandbox Toolkit</h1>
  <p><strong>A single binary that gives coding agents everything they need to work with a remote sandbox.</strong></p>

  [Guide](./docs/usage.md) ·
  [TypeScript SDK](./sdk/typescript) ·
  [Protocol extensions](./rfcs) ·
  [Benchmarks](./docs/benchmark-report.md)

  [![crates.io](https://img.shields.io/crates/v/sbx)](https://crates.io/crates/sbx)
  [![Build & Test](https://github.com/OpenInsightDev/sandbox-toolkit/actions/workflows/build+test.yaml/badge.svg)](https://github.com/OpenInsightDev/sandbox-toolkit/actions/workflows/build+test.yaml)
  [![MIT](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE)
</div>

## The missing layer between an agent and a sandbox

A coding agent needs more than a way to read one file. To work effectively it
needs to explore a workspace, search and query its contents, make precise
changes, and coordinate those changes with the environment around it.

Most sandbox providers expose only a small set of file primitives. For example,
[Daytona's file-system operations](https://www.daytona.io/docs/en/file-system-operations/)
are representative of the kind of basic API a sandbox may provide. Operations
that coding agents routinely rely on—such as reading a selected range of lines,
searching a workspace, or making a precise partial update—are often missing
from the sandbox API, even though the agent itself already knows how to use
them. This creates a capability mismatch between the agent and the sandbox.

Sandbox Toolkit closes that gap. It runs as one small binary in or next to a
remote sandbox and gives a local coding agent a consistent, general-purpose
interface to that sandbox:

```text
local coding agent  <── Sandbox Toolkit ──>  remote sandbox
```

The agent can stay where it is. The sandbox only needs the toolkit and the
workspace-specific dependencies it already has. There is no need to install
the agent itself inside the sandbox, and no need to adapt the agent to a
provider-specific API such as a single `read_file` operation.

## What it provides

### A complete workspace interface

Sandbox Toolkit solves this mismatch with a general-purpose extension of the
WebDAV protocol. To an agent, the remote workspace behaves like a normal
working directory: it can inspect files and directories, read and write
content, create paths, move or copy resources, and remove them.

The extension adds the richer operations agents need—such as reading only the
relevant lines of a text file and updating only the relevant part of a large
file—without requiring every sandbox provider to invent and maintain its own
private API. The result is one reusable interface between agents and sandboxes,
with less data transfer and no loss of the simple workspace model.

WebDAV provides a standard, widely supported foundation, but it is not the
product boundary. The same endpoint can be used by an agent client, a custom
HTTP integration, or a regular WebDAV client for manual inspection.

### The tools agents expect

A fresh sandbox is often intentionally minimal. Sandbox Toolkit packages the
small command-line tools that agents commonly use to understand a codebase:

- **`rg` (ripgrep)** for fast text and regular-expression search
- **`fd`** for fast file discovery
- **`tgrep`** (optional) for tree-sitter-powered structural search
- **`jaq`** (optional) for querying and transforming JSON

This means a new sandbox can be made useful without a long provisioning script
or a separate package-installation step. The default build includes all optional
tools:

```sh
cargo build --release --all-features
```

Docker images include all optional tools by default; use `--build-arg FEATURES=...` only to build a custom subset.

### A deployable sandbox foundation

The toolkit also supplies the practical pieces needed to expose a sandbox
safely and reliably: TLS and HTTP/2, optional Basic Auth, persistent shadow
credentials, health checks, request logging, locking, and conditional updates.
It is designed for containers, VMs, hosted development environments, and other
short-lived or remote execution environments.

## Why this architecture?

Sandbox providers expose different private APIs and often provide only a small
set of file primitives. Building the agent around those APIs makes the agent
hard to reuse and forces every integration to reinvent workspace support.

Sandbox Toolkit moves that responsibility into a small, provider-independent
server. The agent talks to one stable interface, while the sandbox can change
underneath it. This gives local agents the reach of a full remote development
environment without placing the agent runtime in the sandbox.

## Quick start

Pull the image and expose a sandbox directory:

```sh
docker pull ghcr.io/openinsightdev/sandbox-toolkit:latest

docker run --rm -p 8080:8080 \
  -v "$PWD:/mnt/data" \
  ghcr.io/openinsightdev/sandbox-toolkit:latest
```

Or run the binary directly:

```sh
sbx /path/to/sandbox --host 0.0.0.0 --port 8080
```

The sandbox is now available at `http://localhost:8080`. Connect an agent's
Sandbox Toolkit client to that endpoint, or inspect it with any HTTP/WebDAV
client. For credentials, TLS, logging, shadow files, and deployment examples,
see the [usage guide](./docs/usage.md).

## Integrate with an agent

- Use the [TypeScript SDK](./sdk/typescript) when building an agent or
  orchestration service.
- Use the HTTP/WebDAV endpoint from an existing client.
- Use the protocol extensions in [RFC-0001](./rfcs/0001-line-range-get.md)
  and [RFC-0002](./rfcs/0002-webdav-partial-update.md) when an integration
  needs efficient partial reads or writes.

## Documentation

| Document | Description |
| --- | --- |
| [Usage guide](./docs/usage.md) | CLI, authentication, TLS, logging, and health checks |
| [TypeScript SDK](./sdk/typescript) | Client library for agent integrations |
| [Protocol extensions](./rfcs) | Agent-oriented read and update operations |
| [Docker Compose](./docs/deploy-docker-compose.md) | Container deployment |
| [Podman Quadlet](./docs/deploy-podman-quadlet.md) | Podman deployment |
| [Kubernetes](./docs/deploy-k8s.md) | Kubernetes deployment |
| [Benchmark report](./docs/benchmark-report.md) | Performance results |
| [Litmus report](./docs/litmus-test-report.md) | WebDAV compatibility results |

## License

Sandbox Toolkit is available under the [MIT License](./LICENSE).
