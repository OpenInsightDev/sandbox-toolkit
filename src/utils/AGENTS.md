# utils/

## error.rs — OrStatus trait

`OrStatus<T>` trait extends `Result<T, E: Display>`:
- `or_400(msg)`, `or_403(msg)`, `or_404(msg)`, `or_409(msg)`, `or_500(msg)` — convenience methods
- `or_status(code, msg)` — generic, auto-selects log level: 4xx → `debug!`, 5xx → `error!`

`IntoResolved` trait converts `ResolveTargetError → Result<T, StatusCode>`.

## path.rs — Path resolution

Three functions, all percent-decode via `percent_encoding::percent_decode_str`. Invalid UTF-8 → `ResolveTargetError::InvalidPath`.

- `resolve_existing(root_dir, root_canonical, request_path) → Option<PathBuf>` — async, canonicalize + traversal check for read/delete ops
- `resolve_write_target(root_dir, request_path) → Option<PathBuf>` — sync, segment check + traversal guard for write ops (PUT/DELETE/MKCOL). Rejects empty paths, trailing `/`, `..`, `.` segments.
- `resolve_and_guard(root_dir, root_canonical, request_path, canonical_cache) → Result<PathBuf, ResolveTargetError>` — async, target resolution + parent canonicalization (cached) + traversal check. Does NOT create parent directories.

`ResolveTargetError` variants: `InvalidPath`, `ParentCanonicalizeFailed(io::Error)`, `TraversalBlocked`. `status(on_invalid)` maps to StatusCode.

## time.rs

`format_rfc850(time) → String` — HTTP-date formatting for `Last-Modified` / directory listing timestamps.
