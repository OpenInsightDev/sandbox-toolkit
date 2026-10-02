# AGENTS.md

`axum`-based tus 1.0.0 server. Port behavior from `references/tusd` (tusd v2):
methods, headers, status codes and the `OPTIONS` capability set follow
`pkg/handler`, and handler/`DataStore` layering mirrors it. Filestore and lockers
map from `pkg/filestore`, `pkg/memorylocker`, `pkg/filelocker`.

Extensions to implement (per [FileSystem.md](../../docs/design/FileSystem.md)); the
relevant tus 1.0.0 spec sections are mirrored under
[docs/tus-protocol/](docs/tus-protocol/README.md) from
[tus.io/protocols/resumable-upload](https://tus.io/protocols/resumable-upload):

- `creation` — [creation.md](docs/tus-protocol/creation.md)
- `creation-with-upload` — [creation-with-upload.md](docs/tus-protocol/creation-with-upload.md)
- `termination` — [termination.md](docs/tus-protocol/termination.md)
- `concatenation` — [concatenation.md](docs/tus-protocol/concatenation.md)

Core protocol: [core-protocol.md](docs/tus-protocol/core-protocol.md).
Access point and exclusions: [FileSystem.md](../../docs/design/FileSystem.md).

Only port tusd's core protocol logic; skip the features this project doesn't
need.
Keep the observable behavior and layering (handler vs. store), but drop Go type
names and concurrency primitives.

Tests: `cargo test -p tus`, porting the cases in `pkg/handler/*_test.go`.
