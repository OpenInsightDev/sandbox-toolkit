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

## Local fixes that diverge from upstream

These adapt the vendored crate to `docs/design/Process.md` and are deliberately
ahead of the upstream commit above. They are not dropped features: every future
sync must re-apply or re-derive them instead of restoring the vendored behavior.

### Signal termination is reported

The vendored PTY backend flattened portable-pty's `ExitStatus` to
`status.exit_code() as i32`, dropping `ExitStatus::signal()`, so a signal-killed
child was indistinguishable from `exit 1`. `ProcessExit { exit_code, signal }`
now carries both, is threaded through the PTY, pipe and driver backends and their
exit channels, and is exposed via `ProcessHandle::exit_status` / `exit_signal`.

### PTY children receive Linux `PR_SET_PDEATHSIG`

portable-pty's `SlavePty::spawn_command` installs its own `pre_exec` with no
extension hook, so the PTY backend could not install `set_parent_death_signal`
the way the pipe backend does. `pty::spawn_process` now opens the slave from the
master's tty name and spawns with `std::process::Command`, whose `pre_exec`
replicates portable-pty's session / controlling-terminal setup and then calls
`set_parent_death_signal`. Upstream only covers this through the dropped
fd-preserving `spawn_helper` path, so do not "restore" the plain
`spawn_command` call.

## Keeping in sync with upstream

Keep the crate close to upstream: do not reintroduce dropped code, and prefer
adapting callers over diverging from the vendored implementation. The retained
tests run with `cargo test -p pty`.

When upstream changes, diff its `codex-rs/utils/pty` against this crate, port the
relevant commits by hand while skipping the dropped features and preserving the
local fixes, then update the commit recorded at the top.
