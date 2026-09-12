# AGENTS.md

When modifying any module's code, you MUST update the corresponding AGENTS.md.

## Build & Run

```sh
cargo check
cargo build --release
cargo run --release -- ./data -v

# Pre-commit checklist (must produce zero warnings)
cargo fmt && cargo clippy -- -D warnings && cargo test

# Litmus WebDAV compliance
cargo run --release -- ./data -vv
TESTS="basic http copymove locks props" TESTROOT=. ./litmus http://localhost:8080

# Benchmarks (6 suites, 52 benchmarks)
cargo bench
cargo bench --bench webdav
cargo bench -- "GET/tiny"
# Results: target/criterion/report/index.html
```

## Architecture

Single crate `rshs` (binary + library targets). Edition 2024, requires Rust 1.88+.

### Request Dispatch

`.fallback(any(dispatch))` — single entry. `req.method()` → `webdav::Method` → match type-safe constants → handler functions. Unknown → `501 Not Implemented`.

### Middleware Order (last `.layer()` runs first)

`HealthCheck` → `Auth` → `LockEnforce` → `TraceLayer` → handler

### AppState

Shared via `Arc<AppState>`. Fields: `root_dir`, `root_canonical`, `auth_state`, `dead_props`, `locks`, `canonical_cache`, `lock_timeout`.

Conveniences: `resolve_existing()`, `resolve_write_target()`, `resolve_and_guard()`.

### Supported Methods

| Method | Handler | Module |
| ------ | ------- | ------ |
| GET/HEAD | `handle_get_head` | `http.rs` |
| PUT | `handle_put` | `http.rs` |
| DELETE | `handle_delete` | `http.rs` |
| OPTIONS | `handle_options` | `http.rs` |
| PROPFIND | `handle_propfind` | `webdav.rs` |
| MKCOL | `handle_mkcol` | `webdav.rs` |
| COPY | `handle_copy` | `webdav.rs` |
| MOVE | `handle_move` | `webdav.rs` |
| PROPPATCH | `handle_proppatch` | `webdav.rs` |
| LOCK | `handle_lock` | `locks.rs` |
| UNLOCK | `handle_unlock` | `locks.rs` |

### Known Limitations

- Dead properties are in-memory only, lost on restart
- `getetag` uses mtime+size hash, cannot detect same-second changes with identical size
- HTML directory listing is unindented to reduce transfer size
- `#fragment` stripped before routing by hyper/axum (RFC 7230 §5.1)

## Testing

- Unit tests in `src/` (`#[cfg(test)]`), integration tests in `tests/`
- External crates reference via `rshs` crate (not relative paths)
- `debug_assert!` for internal call-site invariants (compiled away in release)
- Benchmarks use `tower::ServiceExt::oneshot()` against `make_router()`, no TCP binding

## Authentication

```sh
rshs --user admin:secret --user viewer:public ./data
RSHS_USERS="admin:secret;viewer:public" rshs ./data
```

Shadow files (SHA-512 crypt):
```sh
rshs -S ./shadow --user admin:secret ./data
rshs -S /etc/rshs/shadow:rw -W --user admin:newpass ./data
```

- Shadow path suffix: `:rw` (default) or `:ro`
- `-W` writes CLI credentials to shadow file (requires writable)
- `--auth-cache-ttl` / `RSHS_AUTH_CACHE_TTL`: default 60s, `0` = disabled

## TLS

```sh
rshs --tls-cert cert.pem --tls-key key.pem ./data
```

- Default port 8443 when TLS enabled (unless `--port` set)
- SHA-256 fingerprint logged at startup
- Both cert and key must be explicitly provided

## Logging

```sh
rshs              # info
rshs -v           # debug
rshs -vv          # trace
rshs -q           # silent
RSHS_LOG="rshs[status=500]=debug" rshs  # EnvFilter
```

`RSHS_LOG_STYLE`: `auto` (default), `always`, `never`

## Environment Variables

| Variable | Description |
| -------- | ----------- |
| `RSHS_ROOT_DIR` | Root directory (default `.`) |
| `RSHS_HOST` | Bind address |
| `RSHS_PORT` | Bind port |
| `RSHS_TLS_CERT` | TLS certificate path (PEM) |
| `RSHS_TLS_KEY` | TLS private key path (PEM) |
| `RSHS_USERS` | Basic Auth credentials |
| `RSHS_LOG` | Log level (EnvFilter) |
| `RSHS_LOG_STYLE` | `auto` / `always` / `never` |
| `RSHS_SHADOW_FILE` | Shadow file path, optional `:rw`/`:ro` |
| `RSHS_LOCK_TIMEOUT` | Lock timeout seconds (default 300) |
| `RSHS_AUTH_CACHE_TTL` | Auth cache TTL seconds (default 60, 0=disabled) |
