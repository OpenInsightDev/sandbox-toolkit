# Usage

## Docker Quick Start

```sh
# Serve the current directory
docker run --rm -p 8080:8080 -v .:/mnt/data mogeko/sbx

# Serve a specific directory
docker run --rm -p 8080:8080 -v /path/to/serve:/mnt/data mogeko/sbx

# Custom host and port
docker run --rm -p 3000:3000 -v .:/mnt/data mogeko/sbx --port 3000

# With TLS
docker run --rm -p 8443:8443 \
  -v ./certs:/certs -v .:/mnt/data \
  mogeko/sbx --tls-cert /certs/cert.pem --tls-key /certs/key.pem
```

## Authentication

Basic HTTP Authentication (RFC 7617) is supported via `--user` / `-u` and `SBX_USERS` env var.

```sh
# Single user
docker run --rm -p 8080:8080 -v ./private:/mnt/data mogeko/sbx --user admin:secret123

# Multiple users
docker run --rm -p 8080:8080 -v ./private:/mnt/data \
  mogeko/sbx --user admin:secret --user viewer:public

# Using environment variables
docker run --rm -p 3000:3000 \
  -e SBX_USERS="admin:secret;viewer:public" \
  -v .:/mnt/data \
  mogeko/sbx
```

Credentials format: `username:password`, separated by `;` for multiple users.
CLI values take precedence over environment variables.
If no users are configured, the server runs without authentication.

## Shadow Files

Shadow files provide persistent credential storage with SHA-512 crypt hashing:

```sh
# Load credentials from a shadow file
sbx -S ./shadow ./data

# Read-only shadow file (no writes allowed)
sbx -S /etc/sbx/shadow:ro ./data

# Merge CLI credentials and write to shadow file
sbx -S ./shadow:rw -W --user admin:newpass ./data

# Using environment variable
SBX_SHADOW_FILE=./shadow:ro sbx ./data
```

- Path suffix `:rw` (default) allows writes, `:ro` restricts to read-only
- `-W` / `--shadow-write` writes CLI credentials into the shadow file after merging
- Shadow files store passwords as SHA-512 crypt hashes (`$6$...`), compatible with Unix shadow file format

### Auth Caching

Auth caching reduces repeated SHA-512 crypt verification overhead for returning clients:

```sh
# Default TTL: 60 seconds
sbx --user admin:secret ./data

# Custom TTL: 120 seconds
sbx --user admin:secret --auth-cache-ttl 120 ./data

# Disable caching (re-verify every request)
sbx --user admin:secret --auth-cache-ttl 0 ./data

# Via environment variable
SBX_AUTH_CACHE_TTL=120 sbx --user admin:secret ./data
```

- Only successful authentications are cached — failed attempts always re-verify
- Cache hits refresh the TTL (sliding expiration): frequently-used credentials never expire as long as requests arrive within the TTL window
- Cache entries expire after TTL and are pruned every 30s
- Password changes take effect after at most `auth_cache_ttl` seconds

> [!TIP]
> Since the SBX shadow file is compatible with Unix shadow. You can mount the Linux shadow file as read-only into the container. This allows you to use existing system credentials for authentication.
>
> ```sh
> docker run --rm -p 8080:8080 \
>   -e SBX_SHADOW_FILE=/etc/sbx/shadow:ro \
>   -v /etc/shadow:/etc/sbx/shadow:ro \
>   mogeko/sbx
> ```

## TLS / HTTPS

TLS is enabled by providing both a certificate and private key file in PEM format.
When TLS is active, the default port switches from 8080 to 8443.

```sh
# Generate a self-signed certificate
openssl req -x509 -newkey rsa:4096 -keyout key.pem -out cert.pem \
  -days 365 -nodes -subj "/CN=localhost"

# Start with TLS
sbx --tls-cert cert.pem --tls-key key.pem ./data

# Using environment variables
SBX_TLS_CERT=cert.pem SBX_TLS_KEY=key.pem sbx ./data

# Override default port
sbx --tls-cert cert.pem --tls-key key.pem --port 443 ./data
```

- Default port: 8443 (TLS) vs 8080 (plain) — `--port` always overrides
- HTTP/2 is negotiated via ALPN (`h2` + `http/1.1`)

## Access

| Client           | How to access                                                                 |
| ---------------- | ----------------------------------------------------------------------------- |
| Browser          | Open `http://localhost:8080` (or `https://localhost:8443` with TLS)           |
| macOS Finder     | `Cmd+K` → `http://localhost:8080`                                             |
| Windows Explorer | Map network drive → `http://localhost:8080`                                   |
| Linux davfs2     | `mount.davfs http://localhost:8080 /mnt`                                      |
| curl             | `curl http://localhost:8080` (GET) / `curl -X PROPFIND http://localhost:8080` |

## WebDAV Features

sbx supports WebDAV Class 2 with in-memory locking:

- **Lock/Unlock**: LOCK and UNLOCK with token validation
- **Lock enforcement**: Modification operations (PUT, DELETE, etc.) are rejected if locked by another principal
- **Lock discovery**: PROPFIND with `lockdiscovery` property
- **Conditional requests**: `If` header enforcement for lock tokens
- **Copy/Move**: COPY and MOVE with proper destination handling
- **Lock timeout**: Default 300s timeout when client omits the `Timeout` header.
  Use `0` for unlimited. Configurable via `--lock-timeout`.

Locks are ephemeral (lost on server restart) and stored in memory. Full litmus conformance
results are available in the [Litmus Test Report](./litmus-test-report.md).

## Health Check

sbx provides a header-based health check endpoint that avoids path conflicts
with served files. Any request with `x-health-check: true` header returns
`200 OK`, regardless of the URL path:

```sh
# Health check (no auth required)
curl -H "x-health-check: true" http://localhost:8080/
# → OK

# Works at any path
curl -H "x-health-check: true" http://localhost:8080/subdir/deep
# → OK
```

The health check runs before authentication, so it always works even when
auth is enabled. Requests are logged at `debug` level.

## Logging

sbx uses the [`tracing`](https://crates.io/crates/tracing) ecosystem for structured, span-based logging. Log level is determined by the following priority (highest first):

1. `-q` / `--quiet` — suppress all logs (`off`)
2. `-vv` / `--verbose --verbose` — trace level
3. `-v` / `--verbose` — debug level
4. `SBX_LOG` env var — filter string (e.g. `info`, `sbx=debug`, `sbx[status=500]=trace`)
5. Default — `info` level

```sh
# Default: info level
docker run --rm -p 8080:8080 mogeko/sbx

# Debug level
docker run --rm -p 8080:8080 mogeko/sbx -v

# Trace level (most verbose)
docker run --rm -p 8080:8080 mogeko/sbx -vv

# Suppress all logs
docker run --rm -p 8080:8080 mogeko/sbx -q

# Using environment variable for log level
docker run --rm -p 8080:8080 -e SBX_LOG="debug" mogeko/sbx
```

### Structured Filtering

`SBX_LOG` supports per-target and per-field filtering via [tracing's `EnvFilter`:](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html)

```sh
# Only show sbx logs at debug level, everything else at warn
SBX_LOG="warn,sbx=debug" sbx ./data

# Only show 500 errors from sbx at debug level
SBX_LOG="sbx[status=500]=debug" sbx ./data

# Trace only requests for .html files
SBX_LOG="sbx[path*=*.html]=trace" sbx ./data
```

The following fields are available for filtering:

| Field    | Source                 | Values                          |
| -------- | ---------------------- | ------------------------------- |
| `status` | HTTP response status   | `200`, `404`, `405`, `500`      |
| `method` | HTTP method            | `GET`, `HEAD`, `PROPFIND`, etc. |
| `path`   | Request path           | e.g. `/docs/readme.md`          |
| `user`   | Authenticated username | e.g. `admin`                    |

`SBX_LOG_STYLE` controls log output ANSI color (`auto`, `always`, `never`).

## CLI Reference

```plaintext
A hybrid HTTP file server and WebDAV server

Usage: sbx [OPTIONS] [ROOT_DIR]

Arguments:
  [ROOT_DIR]  Root directory to serve [env: SBX_ROOT_DIR=] [default: .]

Options:
  -H, --host <HOST>
          Host address to bind to [env: SBX_HOST=] [default: 0.0.0.0]
  -p, --port <PORT>
          Port to bind to (default: 8080, or 8443 with TLS) [env: SBX_PORT=]
      --tls-cert <TLS_CERT>
          TLS certificate file path (PEM format) [env: SBX_TLS_CERT=]
      --tls-key <TLS_KEY>
          TLS private key file path (PEM format) [env: SBX_TLS_KEY=]
  -u, --user <USER:PASS>
          Basic Auth credentials as username:password (repeatable) [env: SBX_USERS]
  -S, --shadow-file <PATH[:rw|:ro]>
          Shadow file for persistent SHA-512 credentials (PATH[:rw|:ro], default :rw) [env: SBX_SHADOW_FILE=]
  -W, --shadow-write
          Write CLI credentials into the shadow file
      --auth-cache-ttl <AUTH_CACHE_TTL>
          Auth cache TTL in seconds (0 = disabled) [env: SBX_AUTH_CACHE_TTL=] [default: 60]
      --lock-timeout <LOCK_TIMEOUT>
          WebDAV lock timeout in seconds (0 = never expire) [env: SBX_LOCK_TIMEOUT=] [default: 300]
  -v, --verbose...
          Increase log verbosity (-v = debug, -vv = trace)
  -q, --quiet
          Suppress all log output
  -h, --help
          Print help (see more with '--help')
  -V, --version
          Print version

Logging environment variables:
  SBX_LOG          Tracing filter (e.g. info, sbx=debug, sbx[status=500]=trace)
                    Only used when no -v/-q flags are given
                    Supports per-target and per-field filtering
  SBX_LOG_STYLE    Log style (always, never, auto), controls ANSI color output
                    Defaults to auto (enabled when output is a terminal)
```
