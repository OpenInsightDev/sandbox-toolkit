# AGENTS.md

Vendored from `openai/codex` version `0.0.0` (commit `5b1d656018`) at
`codex-rs/utils/pty`. The port intentionally keeps only Unix (PTY and pipe)
support; Windows (ConPTY) and inherited-file-descriptor support were dropped.

## Retained features

- PTY spawn via `spawn_pty_process` (portable-pty pair, async IO over `AsyncFd`).
- Pipe spawn via `spawn_pipe_process` / `spawn_pipe_process_no_stdin`.
- `ProcessHandle`: stdin writer channel, `resize`, `close_stdin`, `signal`,
  `terminate`, `has_exited`, `exit_code`.
- Process-group lifecycle: new session / `setsid`, interrupt and kill the whole
  group, macOS member fallback, Linux `PR_SET_PDEATHSIG`.
- Split stdout/stderr receivers plus `combine_output_receivers`.
- Driver-backed sessions via `spawn_from_driver`.

## Dropped features

- Windows: ConPTY, `WindowsTtyInputNormalizer`, job objects.
- Inherited file descriptors: `inherited_fds` parameters and fd-preserving spawn.

Keep the crate close to upstream: do not reintroduce dropped code, and prefer
adapting callers over diverging from the vendored implementation. The retained
tests run with `cargo test -p pty`.

When upstream changes, diff its `codex-rs/utils/pty` against this crate, port the
relevant commits by hand while skipping the dropped features, and update the
commit recorded above.
