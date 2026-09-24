//! `sandbox-toolkit` — an Axum HTTP server for running sandboxed tooling.
//!
//! This is the crate root and holds the executable's own concerns: the CLI, the
//! bundled binaries, the shared state, error and HTTP response types, and the
//! logging setup. The server itself lives in [`server`] and the HTTP surface in
//! the per-resource modules such as [`fs`] and [`workspace`].

mod binary;
mod fs;
mod mcp;
mod process;
mod server;
mod workspace;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::Json;
use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};
use clap::Parser;
use serde::Serialize;
use ts_rs::TS;

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();

    let binaries = binary::materialize()
        .await
        .context("failed to materialize the bundled binaries")?;
    tracing::info!(dir = %binaries.display(), "materialized the bundled binaries");

    server::run(Cli::parse()).await
}

/// Install the global tracing subscriber.
///
/// Verbosity is controlled through `RUST_LOG` and falls back to `info` for
/// everything plus `debug` for this crate.
fn init_tracing() {
    use tracing_subscriber::{EnvFilter, fmt};

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,sandbox_toolkit=debug"));

    fmt().with_env_filter(filter).init();
}

/// Command line interface used to configure and start the server.
#[derive(Debug, Clone, Parser)]
#[command(name = "sandbox-toolkit", version, about, long_about = None)]
struct Cli {
    /// Interface address the HTTP server binds to.
    #[arg(
        long,
        value_name = "ADDR",
        default_value_t = IpAddr::V4(Ipv4Addr::LOCALHOST),
        env = "SANDBOX_TOOLKIT_HOST",
    )]
    host: IpAddr,

    /// TCP port the HTTP server listens on.
    #[arg(
        long,
        short,
        value_name = "PORT",
        default_value_t = 3000,
        env = "SANDBOX_TOOLKIT_PORT"
    )]
    port: u16,

    /// Root directory that filesystem routes are confined to.
    #[arg(
        long,
        value_name = "DIR",
        default_value = ".",
        env = "SANDBOX_TOOLKIT_ROOT"
    )]
    root: PathBuf,
}

impl Cli {
    /// Socket address the server should bind to.
    fn socket_addr(&self) -> SocketAddr {
        SocketAddr::new(self.host, self.port)
    }
}

/// State shared by every handler in the service.
///
/// Cloning is cheap: the inner value lives behind an [`Arc`].
#[derive(Debug, Clone)]
struct AppState {
    inner: Arc<AppStateInner>,
}

#[derive(Debug)]
struct AppStateInner {
    root: PathBuf,
    workspaces: workspace::WorkspaceRegistry,
}

impl AppState {
    /// Create a new state rooted at `root`.
    fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            inner: Arc::new(AppStateInner {
                root: root.into(),
                workspaces: workspace::WorkspaceRegistry::default(),
            }),
        }
    }

    /// Root directory that filesystem operations are confined to.
    fn root(&self) -> &Path {
        &self.inner.root
    }

    /// The process-wide workspace registry.
    ///
    /// Every module that addresses a resource within a workspace resolves its
    /// id through this handle.
    fn workspaces(&self) -> &workspace::WorkspaceRegistry {
        &self.inner.workspaces
    }
}

/// Machine readable payload returned for every failed request.
#[derive(Debug, Serialize, TS)]
#[ts(export)]
struct ErrorResponse {
    error: ErrorDetails,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
struct ErrorDetails {
    /// Stable machine-readable classification.
    code: String,
    /// Human-readable description; clients must not branch on this value.
    message: String,
    /// Correlation identifier for logs and support requests.
    request_id: String,
}

/// Scaffolding note: this route surface is intentionally unimplemented.
#[allow(dead_code)]
#[derive(Debug, thiserror::Error)]
enum AppError {
    /// The route exists but its implementation is not wired up yet.
    #[error("not implemented: {0}")]
    NotImplemented(&'static str),

    /// The requested resource does not exist.
    #[error("not found: {0}")]
    NotFound(String),

    /// The request was malformed.
    #[error("bad request: {0}")]
    BadRequest(String),

    /// The resource already exists.
    #[error("conflict: {0}")]
    Conflict(String),

    /// The workspace id falls into a namespace the server manages.
    ///
    /// A separate variant from [`Self::Conflict`] because the machine-readable
    /// code, not just the status, tells the caller why the id is unavailable.
    #[error("workspace id is reserved: {0}")]
    WorkspaceIdReserved(String),

    /// The resource does not support the method the request used.
    ///
    /// The `Allow` header required by HTTP for a 405 is added by axum when this
    /// error is raised from a
    /// [`MethodRouter`](axum::routing::MethodRouter) fallback, which is also the
    /// only place that knows which methods the route registered.
    #[error("method not allowed: {0}")]
    MethodNotAllowed(Method),

    /// A condition carried by the request does not hold.
    ///
    /// Raised when an `If-Match` value no longer matches the current resource
    /// version, which `docs/design/FileSystem.md` maps to `412 Precondition
    /// Failed`.
    #[error("precondition failed: {0}")]
    PreconditionFailed(String),

    /// The request needs a condition it did not carry.
    ///
    /// Raised when an operation that must be conditional, such as overwriting or
    /// deleting an existing resource, arrives without `If-Match`, which
    /// `docs/design/FileSystem.md` maps to `428 Precondition Required`.
    #[error("precondition required: {0}")]
    PreconditionRequired(String),

    /// An unexpected internal failure.
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl AppError {
    /// HTTP status that represents this error.
    fn status(&self) -> StatusCode {
        match self {
            Self::NotImplemented(_) => StatusCode::NOT_IMPLEMENTED,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Conflict(_) | Self::WorkspaceIdReserved(_) => StatusCode::CONFLICT,
            Self::MethodNotAllowed(_) => StatusCode::METHOD_NOT_ALLOWED,
            Self::PreconditionFailed(_) => StatusCode::PRECONDITION_FAILED,
            Self::PreconditionRequired(_) => StatusCode::PRECONDITION_REQUIRED,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// Stable machine-readable classification, mirroring [`Self::status`].
    fn code(&self) -> &'static str {
        match self {
            Self::NotImplemented(_) => "not_implemented",
            Self::NotFound(_) => "not_found",
            Self::BadRequest(_) => "bad_request",
            Self::Conflict(_) => "conflict",
            Self::WorkspaceIdReserved(_) => "workspace_id_reserved",
            Self::MethodNotAllowed(_) => "method_not_allowed",
            Self::PreconditionFailed(_) => "precondition_failed",
            Self::PreconditionRequired(_) => "precondition_required",
            Self::Internal(_) => "internal_error",
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status();

        if status.is_server_error() {
            tracing::error!(error = %self, "request failed");
        }

        let body = Json(ErrorResponse {
            error: ErrorDetails {
                code: self.code().to_owned(),
                message: self.to_string(),
                request_id: "req_unassigned".to_owned(),
            },
        });

        (status, body).into_response()
    }
}
