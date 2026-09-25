//! An Axum HTTP server for running sandboxed tooling.

mod binary;
mod fs;
mod mcp;
mod process;
mod server;
mod skill;
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

fn init_tracing() {
    use tracing_subscriber::{EnvFilter, fmt};

    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,sbxkit=debug"));

    fmt().with_env_filter(filter).init();
}

#[derive(Debug, Clone, Parser)]
#[command(name = "sbxkit", version, about, long_about = None)]
struct Cli {
    /// Interface address the HTTP server binds to.
    #[arg(
        long,
        value_name = "ADDR",
        default_value_t = IpAddr::V4(Ipv4Addr::LOCALHOST),
        env = "SBXKIT_HOST",
    )]
    host: IpAddr,

    /// TCP port the HTTP server listens on.
    #[arg(
        long,
        short,
        value_name = "PORT",
        default_value_t = 3000,
        env = "SBXKIT_PORT"
    )]
    port: u16,

    /// Root directory that filesystem routes are confined to.
    #[arg(long, value_name = "DIR", default_value = ".", env = "SBXKIT_ROOT")]
    root: PathBuf,
}

impl Cli {
    fn socket_addr(&self) -> SocketAddr {
        SocketAddr::new(self.host, self.port)
    }
}

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
    fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            inner: Arc::new(AppStateInner {
                root: root.into(),
                workspaces: workspace::WorkspaceRegistry::default(),
            }),
        }
    }

    fn root(&self) -> &Path {
        &self.inner.root
    }

    fn workspaces(&self) -> &workspace::WorkspaceRegistry {
        &self.inner.workspaces
    }
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
struct ErrorResponse {
    error: ErrorDetails,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
struct ErrorDetails {
    code: String,
    /// Clients must not branch on this value.
    message: String,
    request_id: String,
}

/// Scaffolding note: this route surface is intentionally unimplemented.
#[allow(dead_code)]
#[derive(Debug, thiserror::Error)]
enum AppError {
    #[error("not implemented: {0}")]
    NotImplemented(&'static str),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("target is not a file: {0}")]
    NotAFile(String),

    #[error("target is not a directory: {0}")]
    NotADirectory(String),

    /// The `Allow` header is added by axum only when the error comes from a
    /// [`MethodRouter`](axum::routing::MethodRouter) fallback, which is the one
    /// place that knows the registered methods.
    #[error("method not allowed: {0}")]
    MethodNotAllowed(Method),

    #[error("precondition failed: {0}")]
    PreconditionFailed(String),

    #[error("precondition required: {0}")]
    PreconditionRequired(String),

    /// Separate from [`Self::BadRequest`] so the machine-readable code tells a
    /// malformed body apart from a malformed path or query.
    #[error("invalid request: {0}")]
    InvalidRequest(String),

    #[error("unsupported type: {0}")]
    UnsupportedType(String),

    #[error("invalid MCP configuration: {0}")]
    InvalidMcp(String),

    #[error("unsupported MCP transport: {0}")]
    UnsupportedTransport(String),

    #[error("workspace is in use: {0}")]
    WorkspaceInUse(String),

    #[error("workspace is read-only: {0}")]
    ReadOnlyWorkspace(String),

    #[error("workspace is managed: {0}")]
    ManagedWorkspace(String),

    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl AppError {
    fn status(&self) -> StatusCode {
        match self {
            Self::NotImplemented(_) => StatusCode::NOT_IMPLEMENTED,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::BadRequest(_) | Self::NotAFile(_) | Self::NotADirectory(_) => {
                StatusCode::BAD_REQUEST
            }
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::MethodNotAllowed(_) => StatusCode::METHOD_NOT_ALLOWED,
            Self::PreconditionFailed(_) => StatusCode::PRECONDITION_FAILED,
            Self::PreconditionRequired(_) => StatusCode::PRECONDITION_REQUIRED,
            Self::InvalidMcp(_)
            | Self::UnsupportedTransport(_)
            | Self::InvalidRequest(_)
            | Self::UnsupportedType(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Self::WorkspaceInUse(_) => StatusCode::CONFLICT,
            Self::ReadOnlyWorkspace(_) | Self::ManagedWorkspace(_) => StatusCode::FORBIDDEN,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn code(&self) -> &'static str {
        match self {
            Self::NotImplemented(_) => "not_implemented",
            Self::NotFound(_) => "not_found",
            Self::BadRequest(_) => "bad_request",
            Self::NotAFile(_) => "not_a_file",
            Self::NotADirectory(_) => "not_a_directory",
            Self::Conflict(_) => "conflict",
            Self::MethodNotAllowed(_) => "method_not_allowed",
            Self::PreconditionFailed(_) => "precondition_failed",
            Self::PreconditionRequired(_) => "precondition_required",
            Self::InvalidMcp(_) => "invalid_mcp",
            Self::UnsupportedTransport(_) => "unsupported_transport",
            Self::InvalidRequest(_) => "invalid_request",
            Self::UnsupportedType(_) => "unsupported_type",
            Self::WorkspaceInUse(_) => "workspace_in_use",
            Self::ReadOnlyWorkspace(_) => "read_only_workspace",
            Self::ManagedWorkspace(_) => "managed_workspace",
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
