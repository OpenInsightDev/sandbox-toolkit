# Vite+ Monorepo Starter

A starter for creating a Vite+ monorepo, orchestrated with
[Turborepo](https://turborepo.dev). Turborepo also drives the Rust crates through
its experimental Cargo workspace support.

## Development

- Check everything is ready:

```bash
pnpm run ready
```

- Build every package and crate:

```bash
pnpm run build
```

- Run the tests:

```bash
pnpm run test
```

- Check types, lint, and formatting:

```bash
pnpm run check
```

- Run the development servers and watchers:

```bash
pnpm run dev
```

These scripts are thin wrappers around `turbo run <task>`. You can call Turborepo
directly for anything the scripts do not cover:

```bash
pnpm exec turbo run build --filter=@sandbox-toolkit/sdk
pnpm exec turbo run test --filter=sandbox-toolkit
pnpm exec turbo run dev
```

## Layout

- `packages/sdk` — the TypeScript SDK, built and tested with Vite+.
- `crates/server` — the Rust server crate, part of the root Cargo workspace.
- `turbo.json` — task graph and caching configuration.

## Server

The `crates/server` binary starts an axum HTTP server. Every filesystem
operation is served by one streamable HTTP endpoint, `/fs`, which speaks
JSON-RPC 2.0:

```bash
cargo run --package sandbox-toolkit -- --host 127.0.0.1 --port 3000
```

```bash
curl -s http://127.0.0.1:3000/fs \
  -H 'content-type: application/json' \
  -H 'accept: application/json, text/event-stream' \
  --data '{"jsonrpc":"2.0","id":1,"method":"fs/readFile","params":{"path":"Cargo.toml","offset":0,"limit":10}}'
```

The endpoint answers `text/event-stream` (one SSE `message` event) when the
client accepts it and `application/json` otherwise; notifications get
`202 Accepted` with no body. `fs/readFile` takes `path`, `offset` (zero-based
first line) and `limit` (line count), and returns the requested lines with
their line numbers, whether the window was `truncated`, and the `next_offset`
to continue from.

## Container images

The `Containerfile` packages the binaries the release build already produced
into two images, and `.github/workflows/release.yml` publishes both to GHCR for
`linux/amd64` and `linux/arm64` whenever a `v*` tag is pushed (or the workflow
is dispatched by hand):

| Image                                           | Contents                                                           |
| ----------------------------------------------- | ------------------------------------------------------------------ |
| `ghcr.io/openinsightdev/sandbox-toolkit:latest` | Debian 13 with the server on `PATH`, running as the `sandbox` user |
| `ghcr.io/openinsightdev/sandbox-toolkit:bin`    | the release binary and nothing else, for `COPY --from`             |

The `:latest` image is the smallest environment the bundled `uv` and `deno`
can run in: it carries the glibc they are linked against, a CA trust store
for reaching their package registries, and a writable home for their caches.

```bash
docker run --rm -p 8000:8000 ghcr.io/openinsightdev/sandbox-toolkit
```

The `:bin` image is `FROM scratch` and holds one glibc-linked executable, so
it is only useful as the source of a `COPY --from` in another glibc-based
image:

```dockerfile
COPY --from=ghcr.io/openinsightdev/sandbox-toolkit:bin /sandbox-toolkit /usr/local/bin/sandbox-toolkit
```

Packages published to GHCR start out private; to let others pull these images,
switch the package's visibility to public in its GitHub package settings.

Images are assembled from a prebuilt binary rather than compiled, so a local
build needs a Linux binary staged at `dist/linux/<arch>/sandbox-toolkit`:

```bash
mkdir -p dist/linux/amd64
cp target/x86_64-unknown-linux-gnu/release/sandbox-toolkit dist/linux/amd64/
docker build -f Containerfile --target runtime -t sandbox-toolkit .
docker build -f Containerfile --target bin -t sandbox-toolkit-bin .
```

## Cargo tasks

`turbo.json` enables `futureFlags.experimentalCargoWorkspaces`, so Turborepo
discovers the crates in the root `Cargo.toml` workspace and maps common task
names to Cargo commands:

| Task     | Cargo command                                             |
| -------- | --------------------------------------------------------- |
| `build`  | `cargo build --package=<crate> --locked --features tools` |
| `test`   | `cargo test --workspace --locked --features tools`        |
| `check`  | `cargo check --workspace --locked --features tools`       |
| `lint`   | `cargo clippy --workspace --locked --features tools`      |
| `format` | `cargo fmt --all`                                         |
| `dev`    | `cargo run --package=<crate> --locked`                    |

The `tools` feature is the umbrella that enables every bundled tool (`jaq`,
`tgrep`, `uv`, `deno`), so the checks exercise what actually ships instead of a
build with no tools. Turborepo runs its Cargo tasks without any feature by
default, so `turbo.json` overrides these commands to pass `--features tools`; add
a new tool to the `tools` list in `crates/server/Cargo.toml` and nothing else
needs to change.

This replaces the Cargo integration that `pnpm` used to provide through
`cargo.enabled`. `pnpm install` now installs npm packages only, Cargo resolves
crates from the registry as usual, and Turborepo caches and schedules both
halves of the repository from one task graph.
