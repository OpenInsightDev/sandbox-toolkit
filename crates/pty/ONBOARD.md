# ONBOARD

Issues to fix inside this crate so it can carry the pty semantics of
`docs/design/Process.md`.

## The PTY backend sets no `PR_SET_PDEATHSIG`

The pipe backend installs `set_parent_death_signal` in `pre_exec`; the PTY backend
does not, so a PTY child survives the server's death on Linux.
