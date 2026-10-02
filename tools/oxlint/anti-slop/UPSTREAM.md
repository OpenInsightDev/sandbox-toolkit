# Vendored anti-slop plugin

Source: [dmmulroy/anti-slop](https://github.com/dmmulroy/anti-slop), commit `c44ef22ca116d0ba62a3ff663a0bd13a3f3fa40b` (2026-09-10).

Copied from the repository's `src/` tree via its bundled `skills/install-anti-slop/scripts/install.mjs` script. The `effect/` subtree is the opt-in Effect plugin.

## Installed paths

- `tools/oxlint/anti-slop/index.ts` — generic plugin entry point registered as `anti-slop`
- `tools/oxlint/anti-slop/effect/index.ts` — Effect plugin entry point registered as `anti-slop-effect`
- `tools/oxlint/anti-slop/shared/`, `tools/oxlint/anti-slop/rules/`, `tools/oxlint/anti-slop/effect/` — rule and helper sources
- `tools/oxlint/anti-slop/vendor/eslint-stylistic/` — padding-line engine vendored from ESLint Stylistic (MIT), with its `LICENSE` and `UPSTREAM.md` provenance

Rule sources, shared helpers, and the vendored Stylistic files are copied verbatim; `.test.ts` files are omitted because the vendored copy is not executed by this repository's test setup.

## Configuration

Registered in `vite.config.ts` under `lint.jsPlugins`, with all generic rules enabled at `error` plus the native `oxc/no-accumulating-spread` companion. The Effect rule group is enabled because `effect` is a direct dependency in `package.json`. Both `lint.ignorePatterns` and `fmt.ignorePatterns` exclude `tools/oxlint/anti-slop/**` and installed agent tooling so `vp check` leaves vendored assets alone.

A `lint.overrides` entry turns the anti-slop rule group off under `tests/**`. Test code builds tagged fixtures and narrows values ad hoc, which the policy rejects in application code. Test files stay in lint scope so `vp check` still type-checks them.

The plugin imports `@oxlint/plugins`, pinned to `1.79.0` to match the Oxlint version resolved through `vite-plus` `0.3.0`.

## Local deviations

None. Rules and configuration have not been modified from upstream.

## Updating

Fetch an explicit upstream revision, re-run its skill install script into a staging directory, and diff against `tools/oxlint/anti-slop/` before replacing files. Preserve the `vendor/eslint-stylistic/LICENSE` and `UPSTREAM.md`, and update the commit recorded above. Do not force-replace without reviewing differences against any local edits made in the meantime.
