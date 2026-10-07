# AGENTS.md

## Rules

- `tests/` holds integration tests that drive a real server over HTTP; a unit test lives beside the module it covers.
- A test observes behavior through the SDK and the server's HTTP API, and nothing else.
- These tests are not paired with a design doc.

## Test harness

- `harness.ts` is where the suite starts sandboxes, ports and fixtures; a test file adds test points only.
- `startSandbox()` returns a running `sbxtkt serve` at `baseUrl`, and `workspaceLayer(baseUrl)` wires the SDK to it.
- The server runs in a Linux container whatever the host compiles the tests, so a run needs a container runtime and the binary `rust:build:linux` publishes.

## Running

- `vp run test` in this package builds that binary first, then runs the suite.
- A Linux host compiles it directly; any other host cross-compiles it, which needs `zig` and `cargo-zigbuild` plus the `aarch64-unknown-linux-gnu` Rust target.
