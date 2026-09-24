//! Server bootstrap, TCP listener and the MCP surface.

use std::sync::Arc;

use anyhow::{Context, Result};
use axum::Router;
use axum::http::Uri;
use axum::routing::any_service;
use rmcp::ErrorData;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ServerHandler, tool_handler};
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;
use tracing::info;

use crate::{AppError, AppState, Cli, fs, mcp, process, workspace};

/// Build the API router.
fn api_router() -> Router<AppState> {
    Router::new()
        .merge(workspace::router())
        .merge(fs::router())
        .merge(process::router())
        .merge(mcp::router())
        // The MCP transport answers `POST` for JSON-RPC calls, `GET` for the
        // server-to-client stream and `DELETE` to end a session, so it owns every
        // method on the path.
        .route("/mcp", any_service(mcp_service()))
        // Every error response carries the JSON envelope described in
        // `docs/design/FileSystem.md`, including requests that match no route.
        .fallback(not_found)
}

/// Fallback for requests that match no route.
async fn not_found(uri: Uri) -> AppError {
    AppError::NotFound(uri.path().to_owned())
}

/// A session-scoped Streamable HTTP service over [`ToolkitServer`].
fn mcp_service() -> StreamableHttpService<ToolkitServer, LocalSessionManager> {
    StreamableHttpService::new(
        || Ok(ToolkitServer),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    )
}

/// The tool surface of the toolkit.
///
/// Cheap to clone because the transport builds one per session.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ToolkitServer;

impl ToolkitServer {
    /// Every tool the toolkit exposes, gathered from the module that owns it.
    fn tool_router() -> ToolRouter<Self> {
        Self::workspace_tools() + Self::file_tools() + Self::process_tools()
    }
}

#[tool_handler(name = "sandbox-toolkit")]
impl ServerHandler for ToolkitServer {}

/// Error for a tool whose backing operation is not wired up yet.
pub(crate) fn not_implemented(tool: &'static str) -> ErrorData {
    ErrorData::internal_error(format!("not implemented: {tool}"), None)
}

/// Run the HTTP server until it is shut down.
pub(crate) async fn run(cli: Cli) -> Result<()> {
    let state = AppState::new(cli.root.clone());
    let addr = cli.socket_addr();
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind HTTP listener to {addr}"))?;

    info!(%addr, root = %state.root().display(), "sandbox-toolkit server listening");

    let app = api_router()
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    // Without graceful shutdown, the returned future's output is uninhabited:
    // it never resolves and surfaces no error.
    match axum::serve(listener, app).await {}
}
