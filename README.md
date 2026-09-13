<div align="center">
  <h1>SBX</h1>
</div>

<div align="center">
  <h3>WebDAV, simplified</h3>
  <a href="https://github.com/mogeko/sbx/blob/master/docs/usage.md">Guide</a> •
  <a href="https://docs.rs/sbx">API&nbsp;Docs</a> •
  <a href="https://github.com/mogeko/sbx/blob/master/docs/benchmark-report.md">Benchmark</a> •
  <a href="https://github.com/mogeko/sbx/blob/master/docs/litmus-test-report.md">Litmus&nbsp;Test</a> •
  <a href="https://www.rfc-editor.org/info/rfc4918">RFC&nbsp;4918</a>
</div>

<br/>

<div align="center">

[![crates.io](https://img.shields.io/crates/v/sbx)](https://crates.io/crates/sbx)
[![Build & Test](https://github.com/mogeko/sbx/actions/workflows/build+test.yaml/badge.svg)](https://github.com/mogeko/sbx/actions/workflows/build+test.yaml)
[![MIT](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE)

</div>

A WebDAV server that Just Works — zero config, [litmus 100%](./docs/litmus-test-report.md). Built in Rust for speed and correctness, packaged as a single Docker image for easy deployment.

## Features

- **Browser + WebDAV hybrid** — HTML directory listings for humans, WebDAV protocol for mounting (Finder, Explorer, davfs).
- **Fast and lightweight** — dispatch ~42µs (GET); 10MB read at 835 MiB/s, 1000-file directory listing in 1.7ms, PROPFIND 200 files in 1.1ms. [Full benchmark report](./docs/benchmark-report.md)
- **Zero config, [one command](#quick-start)** — no config files, daemons, or runtime dependencies beyond Docker. Just mount a volume and go.
- **Litmus 102/102** — full [RFC 4918 Class 2](https://www.rfc-editor.org/info/rfc4918) compliance: locks, copy/move, conditional `If` headers, dead properties. All five suites pass.
- **TLS + HTTP/2** — built-in HTTPS with automatic HTTP/2 negotiation.
- **Optional Basic Auth** — per-user credentials with persistent shadow files (SHA-512 crypt).

## Installation

```sh
docker pull docker.io/mogeko/sbx:latest
# or
docker pull ghcr.io/mogeko/sbx:latest
```

## Quick Start

```sh
# Serve ./data on port 8080
docker run --rm -p 8080:8080 -v ./data:/mnt/data mogeko/sbx

# With TLS (default port 8443)
docker run --rm -p 8443:8443 \
  -v ./certs:/certs -v ./data:/mnt/data \
  mogeko/sbx --tls-cert /certs/cert.pem --tls-key /certs/key.pem

# With authentication
docker run --rm -p 8080:8080 -v ./data:/mnt/data \
  mogeko/sbx --user admin:secret123
```

Open `http://localhost:8080` in a browser, or mount as WebDAV:

```sh
# Linux (davfs2)
sudo mount -t davfs http://localhost:8080 /mnt/webdav
# macOS (Finder)
Cmd+K → `http://localhost:8080`
# Windows (Explorer)
Map Network Drive → `http://localhost:8080`
```

## Documentation

| Document                             | Description                                      |
| ------------------------------------ | ------------------------------------------------ |
| [Usage Guide][usage-guide]           | Full usage, auth, shadow files, CLI              |
| [Docker Compose][docker-compose]     | Docker Compose deployment                        |
| [Podman Quadlet][podman-quadlet]     | Podman Quadlet deployment                        |
| [Kubernetes][kubernetes]             | K8s deployment, PVC, Ingress                     |
| [Benchmark Report][benchmark-report] | Performance benchmarks (56 benchmarks, 6 suites) |
| [Litmus Test Report][litmus-report]  | WebDAV conformance test results                  |

[usage-guide]: ./docs/usage.md
[docker-compose]: ./docs/deploy-docker-compose.md
[podman-quadlet]: ./docs/deploy-podman-quadlet.md
[kubernetes]: ./docs/deploy-k8s.md
[benchmark-report]: ./docs/benchmark-report.md
[litmus-report]: ./docs/litmus-test-report.md

## Environment Variables

| Variable            | Description                                           | Default   |
| ------------------- | ----------------------------------------------------- | --------- |
| `SBX_ROOT_DIR`     | Root directory to serve                               | `.`       |
| `SBX_HOST`         | Bind address                                          | `0.0.0.0` |
| `SBX_PORT`         | Bind port (8080 plain, 8443 with TLS)                 | —         |
| `SBX_TLS_CERT`     | TLS certificate file path (PEM)                       | —         |
| `SBX_TLS_KEY`      | TLS private key file path (PEM)                       | —         |
| `SBX_USERS`        | `user:pass;...` auth pairs                            | —         |
| `SBX_SHADOW_FILE`  | Shadow file path                                      | —         |
| `SBX_LOCK_TIMEOUT` | Default WebDAV lock timeout in seconds (default: 300) | `300`     |
| `SBX_LOG`          | Log filter (e.g. `debug`, `sbx[status=500]=trace`)   | —         |
| `SBX_LOG_STYLE`    | Log output style                                      | `auto`    |

## License

MIT License. See [LICENSE](./LICENSE) for details.
