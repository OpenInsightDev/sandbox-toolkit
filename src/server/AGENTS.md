# server/

## mod.rs — AppState, Router, dispatch

`AppState` struct (behind `Arc`): `auth_state`, `root_dir`, `root_canonical`, `dead_props`, `locks`, `canonical_cache`, `lock_timeout`. Provides `resolve_existing()`, `resolve_write_target()`, `resolve_and_guard()` delegates to `utils::path`.

`make_router(state) → Router` — builds the full middleware stack and dispatch. Used by both `start_server` and integration tests (no TCP binding needed).

`dispatch()` — single entry point: converts `req.method()` to `webdav::Method`, matches on type-safe constants → handler functions. Unknown → `501 Not Implemented`.

`start_server(config) → io::Result<()>` — binds TCP (or TLS listener), spawns cleanup task, starts axum server with graceful shutdown.

## cleanup.rs

`cleanup_task(locks, auth_cache, shutdown)` — runs every 30s. Prunes expired locks (`LockInfo::is_expired()`) and auth cache entries (`Instant::now() > expiry`). Stops on `Notify`.

## shutdown.rs

`shutdown_signal()` — async, waits for Ctrl+C or SIGTERM (unix). On non-unix, only Ctrl+C.

## tls.rs

`TlsConfig { cert_path, key_path }` — `load() → io::Result<rustls::ServerConfig>` with ALPN `h2` + `http/1.1`. Logs SHA-256 fingerprint of each cert at startup.

`TlsListener` implements `axum::serve::Listener` — wraps TCP listener + `tokio_rustls::TlsAcceptor`. `poll_accept()` returns `(TlsStream, addr)`.
