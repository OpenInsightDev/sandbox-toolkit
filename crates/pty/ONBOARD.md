# ONBOARD

Issues to fix inside this crate so it can carry the pty semantics of
`docs/design/Process.md`.

## Signal termination is lost

`spawn_pty_process` reports the exit as `status.exit_code() as i32`, dropping
portable-pty's `ExitStatus::signal()`. portable-pty maps a signal-killed child to
`code = 1`, so `SIGKILL`/`SIGTERM` is indistinguishable from `exit 1` and the
design's `failed{message}` terminal status cannot be produced.

Carry the signal alongside the exit code instead of flattening to an `i32`.

## The PTY backend sets no `PR_SET_PDEATHSIG`

The pipe backend installs `set_parent_death_signal` in `pre_exec`; the PTY backend
does not, so a PTY child survives the server's death on Linux.
