//! The toolkit's MCP surface, served at `/mcp` over the Streamable HTTP
//! transport.
//!
//! Resource modules define their own MCP tools, and this module owns only the
//! transport and the final session-scoped server assembly.

use std::sync::Arc;

use axum::Router;
use axum::routing::any_service;
use rmcp::ErrorData;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ServerHandler, tool_handler};

use crate::AppState;

/// Build the MCP router, mounted at `/mcp`.
pub(crate) fn router() -> Router<AppState> {
    Router::new().route("/mcp", any_service(service()))
}

/// A session-scoped Streamable HTTP service over [`ToolkitServer`].
///
/// The transport answers `POST` for JSON-RPC calls, `GET` for the server-to-client
/// stream and `DELETE` to end a session, so it owns every method on the path.
fn service() -> StreamableHttpService<ToolkitServer, LocalSessionManager> {
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
        Self::workspace_tools() + Self::file_tools()
    }
}

#[tool_handler(name = "sandbox-toolkit")]
impl ServerHandler for ToolkitServer {}

/// Error for a tool whose backing operation is not wired up yet.
pub(crate) fn not_implemented(tool: &'static str) -> ErrorData {
    ErrorData::internal_error(format!("not implemented: {tool}"), None)
}
