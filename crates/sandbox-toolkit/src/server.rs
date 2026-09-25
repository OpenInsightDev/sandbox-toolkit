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

use crate::{AppError, AppState, Cli, fs, mcp, process, skill, workspace};

fn api_router() -> Router<AppState> {
    Router::new()
        .merge(workspace::router())
        .merge(fs::router())
        .merge(process::router())
        .merge(mcp::router())
        .merge(skill::router())
        // The MCP transport answers `POST` for JSON-RPC calls, `GET` for the
        // server-to-client stream and `DELETE` to end a session, so it owns every
        // method on the path.
        .route("/mcp", any_service(mcp_service()))
        // Going through a fallback keeps the JSON error envelope for unmatched
        // routes; axum's own 404 would carry an empty body.
        .fallback(not_found)
}

async fn not_found(uri: Uri) -> AppError {
    AppError::NotFound(uri.path().to_owned())
}

fn mcp_service() -> StreamableHttpService<ToolkitServer, LocalSessionManager> {
    StreamableHttpService::new(
        || Ok(ToolkitServer),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    )
}

/// Cheap to clone because the transport builds one per session.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ToolkitServer;

impl ToolkitServer {
    fn tool_router() -> ToolRouter<Self> {
        Self::workspace_tools() + Self::resource_tools() + Self::process_tools() + Self::mcp_tools()
    }
}

#[tool_handler(name = "sandbox-toolkit")]
impl ServerHandler for ToolkitServer {}

pub(crate) fn not_implemented(tool: &'static str) -> ErrorData {
    ErrorData::internal_error(format!("not implemented: {tool}"), None)
}

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
