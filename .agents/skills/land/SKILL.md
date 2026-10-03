---
name: land
description: >-
  Land the current change into this repository: commit it on `main` following the
  repository's commit conventions, verify it with the Vite+ tasks, and push it to
  `origin`. Invoke only when the user has explicitly requested landing the
  change (the Land Changes action, `/land`, or an equivalent request to merge or
  ship the change) — never for review, preparation, running checks, or skill
  installation.
disable-model-invocation: true
metadata:
  delta-action: land
---

# Land

Landing means the change is committed on `main` and present on `origin/main`.

`origin` is `github.com/OpenInsightDev/sandbox-toolkit`. The repository has no CI,
no branch protection, and no PR in its history: `main` carries linear, hand-written
commits, and the push is the landing operation. `gh` is authenticated and the user
has admin, but neither is needed to push.

Leave other checkouts of the repository alone; the user keeps them in sync.

## Preflight

Work from the repository root.

- `git remote -v` resolves `origin` to the URL above and `git branch --show-current`
  is `main`. Land on a different branch only when the change came from one and the
  user says to land there.
- `git status --porcelain` and `git diff` separate the change being landed from
  anything else in the working tree. Ask instead of committing or discarding
  unrelated work.
- `node_modules` missing means `vp install` first (README, “Development”); the
  package test suite cannot run without it, which blocks landing.

## Commit

- Stage the landed paths explicitly. Generated bindings
  (`packages/sandbox-toolkit/src/generated`) and `/target` are gitignored: never
  force-add them, and do not `git add -A`.
- One coherent change per commit, matching the existing history (`git log`): a
  `type(sandbox-toolkit): <imperative English summary>` subject (`feat`, `docs` are
  the types in use), and a body stating the rule or decision the change
  establishes together with the test groups it affects. The existing commits carry
  no trailers and are unsigned, and `commit.gpgsign` is unset: keep that.
- Keep the design-doc pairing intact (`crates/sandbox-toolkit/tests/AGENTS.md`):
  every feature has `X.md` paired with `x.rs`, the doc is the source of truth, a
  test point that disagrees with it is corrected to the doc, and a design change
  lands together with its test-group update. Committing tests that do not yet pass
  is allowed; committing with tests and the design doc out of sync is not. Fix any
  desync toward the doc and say so in the report.
- The Vite+ pre-commit hook (`.vite-hooks/pre-commit`, running `vp staged` as
  configured under `staged` in `vite.config.ts`) formats staged files with
  `vp check --fix` and runs `vp run rust:check` when Rust files are staged. It
  applies only where `core.hooksPath` points at `.vite-hooks`; do not configure
  that here. If it rewrites files, re-read `git status` before finishing.

## Verify

Run both entry points defined under `run.tasks` in `vite.config.ts` (the README’s
“Development” section names the same two):

- `vp run check` — `vp run rust:check` (`cargo clippy --workspace --all-targets`),
  then `vp check`.
- `vp run test` — `vp run rust:test` (`cargo test --workspace`), then
  `vp run -r test` (the package suites; the toolkit’s global setup builds the
  server through `vp run rust:build` first).

Both must pass for the change being landed. With no CI, these are the whole gate. A
check that did not run, is still running, or cannot run is not a pass: report it as
a blocker rather than landing on an assumption.

Failures that are unrelated to the change and already fail on the base commit may
be listed and left failing. Confirm each one is unrelated — it fails without the
change, and the change does not touch its area — then name every failing test in
the report. Anything else stops the landing.

Known instance, to re-confirm on every run: `src/pty` does not exist yet, so
`crates/sandbox-toolkit/tests/pty.rs` fails with `404`s and the package suite’s pty
assertions fail.

## Land

- `git fetch origin`, then confirm `main` fast-forwards `origin/main`
  (`git status -sb`, or `git rev-list --left-right --count origin/main...main`).
- `git push origin main`. Never force-push.
- Resolve rebase or merge conflicts automatically when the intended result is
  clear. Stop and ask only when it is clearly incompatible with upstream: when the
  intended result cannot be determined, or when resolving would overwrite someone
  else’s work.

## Confirm

- `git ls-remote origin main`, or
  `gh api repos/OpenInsightDev/sandbox-toolkit/commits/main --jq .sha`, matches the
  local `HEAD`.
- Report the commit hash, branch, destination, each check with its outcome, every
  accepted pre-existing failure, and anything left unverified. If the push did not
  happen, say plainly that the change has not landed and why.
