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

## Cargo tasks

`turbo.json` enables `futureFlags.experimentalCargoWorkspaces`, so Turborepo
discovers the crates in the root `Cargo.toml` workspace and maps common task
names to Cargo commands:

| Task     | Cargo command                            |
| -------- | ---------------------------------------- |
| `build`  | `cargo build --package=<crate> --locked` |
| `test`   | `cargo test --workspace --locked`        |
| `check`  | `cargo check --workspace --locked`       |
| `lint`   | `cargo clippy --workspace --locked`      |
| `format` | `cargo fmt --all`                        |
| `dev`    | `cargo run --package=<crate> --locked`   |

This replaces the Cargo integration that `pnpm` used to provide through
`cargo.enabled`. `pnpm install` now installs npm packages only, Cargo resolves
crates from the registry as usual, and Turborepo caches and schedules both
halves of the repository from one task graph.
