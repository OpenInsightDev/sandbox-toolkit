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

/// Install the global tracing subscriber.
///
/// Verbosity is controlled through `RUST_LOG` and falls back to `info` for
/// everything plus `debug` for this crate.
fn init_tracing() {
    use tracing_subscriber::{EnvFilter, fmt};

    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,sbxkit=debug"));

    fmt().with_env_filter(filter).init();
}

/// Command line interface used to configure and start the server.
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

    /// The operation needs a file but the target is a directory.
    #[error("target is not a file: {0}")]
    NotAFile(String),

    /// The operation needs a directory but the target is a file.
    #[error("target is not a directory: {0}")]
    NotADirectory(String),

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

    /// The request body does not fit the schema of the `type` it names.
    ///
    /// A separate variant from [`Self::BadRequest`] because the machine-readable
    /// code tells a malformed body apart from a malformed path or query.
    #[error("invalid request: {0}")]
    InvalidRequest(String),

    /// The request names a `type` outside the file API's endpoint table.
    #[error("unsupported type: {0}")]
    UnsupportedType(String),

    /// A registered MCP server configuration failed validation.
    ///
    /// Raised when a `server` object does not satisfy the constraints of the
    /// transport it names, which `docs/design/MCP.md` maps to `422 invalid_mcp`.
    #[error("invalid MCP configuration: {0}")]
    InvalidMcp(String),

    /// The request names an MCP transport the service does not support.
    ///
    /// Raised for a `type` such as `sse`, which `docs/design/MCP.md` maps to
    /// `422 unsupported_transport`.
    #[error("unsupported MCP transport: {0}")]
    UnsupportedTransport(String),

    /// A workspace still has resources mounted that must be removed first.
    ///
    /// Raised when unregistering a workspace that owns MCP entries, which
    /// `docs/design/MCP.md` maps to `409 workspace_in_use`.
    #[error("workspace is in use: {0}")]
    WorkspaceInUse(String),

    /// The workspace is read-only and rejects the requested mutation.
    ///
    /// Raised for a workspace whose `access` property is `read-only`, which
    /// `docs/design/Workspace.md` and `docs/design/FileSystem.md` map to
    /// `403 read_only_workspace`; the workspaces the Skill API derives are
    /// always read-only.
    #[error("workspace is read-only: {0}")]
    ReadOnlyWorkspace(String),

    /// The workspace is derived and managed by another resource.
    ///
    /// Raised when deleting a workspace the Skill API derives, which
    /// `docs/design/Skill.md` maps to `403 managed_workspace`.
    #[error("workspace is managed: {0}")]
    ManagedWorkspace(String),

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

    /// Stable machine-readable classification, mirroring [`Self::status`].
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
