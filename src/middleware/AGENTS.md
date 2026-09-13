# middleware/

Runtime order (outermost first): `HealthCheck` → `Auth` → `LockEnforce` → `TraceLayer` → handler.

## health.rs

`HealthCheck` implements `tower::Layer`. Intercepts requests with header `x-health-check: true` (case-insensitive name, exact value match) → `200 OK` body `OK`. Runs before auth and tracing. Logged at `debug` level.

## auth.rs

`auth_middleware` — validates HTTP Basic Auth against `AuthState`. No-op when `is_empty()`. Parses `Authorization: Basic <b64>` header, decodes, calls `AuthState::validate_cached()`. Returns `Result<Response, Unauthorized>` where `Unauthorized` attaches `WWW-Authenticate: Basic realm="sbx"`.

## lock.rs

`lock_enforce` — intercepts write methods: `PUT`, `PATCH`, `DELETE`, `MKCOL`, `PROPPATCH`, `MOVE`, `COPY`. Converts `req.method()` to `webdav::Method` via `TryFrom`. For `COPY`/`MOVE`, checks both source and destination paths. Calls `webdav::eval_if()` with the request's `If` header against `LockStore`. A direct matching `Lock-Token` authorizes a request only when `If` is absent; it cannot override a false `If` condition. Returns `423 Locked` if no matching token, or `412 Precondition Failed` when `If` is present without any token condition. `PATCH` follows the same target/ancestor lock policy as `PUT`.
