# Standalone Module Docs

## cli.rs

clap derive CLI args. `Cli` struct with builder methods:

- `effective_port()` — 8080 (HTTP) or 8443 (TLS, unless `--port` explicitly set)
- `to_tls_config()` — `Option<TlsConfig>`
- `to_shadow_file_arg()` — `Option<ShadowFileArg>`
- `to_auth_state()` — `AuthState` from `--user` entries
- `log_level()` — `-q`/`-v`/`-vv`/`SBX_LOG` resolution

`ShadowFileArg`: `path` + `writable` (`:rw` default, `:ro` to disable writes). `--shadow-write` with `:ro` file exits with error.

## auth.rs

`AuthState` wraps `HashMap<String, Credential>` + `AuthCache` (`HashMap<u64, Instant>`).

- `Credential` enum: `Plaintext(String)` | `Sha512Crypt(String)`
- `AuthCache` maps hashed Authorization header → expiry `Instant`; sliding TTL on each hit
- Failed attempts never cached
- SHA-512 verification offloaded to `spawn_blocking`
- `validate_cached()` method used by `middleware::auth`

Shadow file: `username:$6$...` (SHA-512 crypt). CLI `--user` credentials merged in, optionally written back with `-W`.

`build_auth_state()` — main entry point: parses shadow file + CLI users, returns `AuthState`.

## html.rs

`generate_dir_listing(dir_path, request_path) → (String, usize)` — async. Reads entries via `scandir::batch_read_dir_entries`, sorts dirs-first then alphabetically, renders HTML with navigable links. Includes `../` parent link for non-root paths. Returns `(html, entry_count)`.

Used by `handlers::http::handle_get_head`.

## scandir.rs

`batch_read_dir_entries(dir) → Vec<DirEntryMeta>` — async. On Linux: io_uring batch `statx`. Other platforms: `spawn_blocking` + `std::fs::read_dir` + `metadata`.

`DirEntryMeta`: `name` (OsString), `is_dir`, `size`, `modified`, `created` (Option).

Used by `html::generate_dir_listing` and `webdav::fs::collect_entries`.
