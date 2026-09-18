//! `sandbox-toolkit` MCP server: the MCP Streamable HTTP endpoint at `/mcp`
//! plus ordinary HTTP routes over the same shared logic.

use std::{fmt, net::SocketAddr, sync::Arc, time::Instant};

use axum::{
    Json, Router,
    extract::{Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use clap::Parser;
use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::wrapper::Parameters,
    model::*,
    tool, tool_handler, tool_router,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod file;
mod model;
mod tools;

use file::ReadFileError;
use model::{
    DescribeToolParams, DescribeToolResult, HealthResult, ListToolsResult, ReadFileParams,
    ReadFileResult,
};
use tools::Tool;

/// Path the MCP endpoint is mounted at.
const MCP_ENDPOINT: &str = "/mcp";

#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Address the server binds to.
    #[arg(long, default_value = "127.0.0.1:8000")]
    bind: SocketAddr,
}

/// State shared by the HTTP routes and the MCP handlers.
#[derive(Clone)]
struct AppState {
    started_at: Instant,
}

impl AppState {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            started_at: Instant::now(),
        })
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Cancelling this token stops every live MCP session.
    let cancellation_token = CancellationToken::new();
    let state = AppState::new();

    // Runs once per MCP session; `AppState` stays shared.
    let mcp = StreamableHttpService::new(
        {
            let state = state.clone();
            move || Ok(SandboxServer::new(state.clone()))
        },
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default()
            .with_cancellation_token(cancellation_token.child_token()),
    );

    let router = Router::new()
        .nest_service(MCP_ENDPOINT, mcp)
        .route("/health", get(http_health))
        .route("/tools", get(http_list_tools))
        .route("/tools/describe", post(http_describe_tool))
        .route("/fs/readFile", post(http_read_file))
        .layer(middleware::from_fn(log_requests))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(args.bind).await?;
    tracing::info!(bind = %args.bind, mcp = MCP_ENDPOINT, "server listening");

    axum::serve(listener, router)
        .with_graceful_shutdown({
            let cancellation_token = cancellation_token.clone();
            async move {
                let _ = tokio::signal::ctrl_c().await;
                cancellation_token.cancel();
            }
        })
        .await?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Shared operations
// ---------------------------------------------------------------------------

impl From<Tool> for DescribeToolResult {
    fn from(tool: Tool) -> Self {
        Self {
            name: tool.name.to_string(),
            path: tool.path.to_string(),
        }
    }
}

fn all_tools() -> ListToolsResult {
    ListToolsResult {
        tools: tools::bundled()
            .into_iter()
            .map(DescribeToolResult::from)
            .collect(),
    }
}

fn find_tool(name: &str) -> Option<DescribeToolResult> {
    tools::find(name).map(DescribeToolResult::from)
}

fn health_result(state: &AppState) -> HealthResult {
    HealthResult {
        status: "ok".to_string(),
        uptime_seconds: state.started_at.elapsed().as_secs(),
    }
}

/// A tool name that matches no bundled executable.
struct UnknownToolError(String);

impl fmt::Display for UnknownToolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "unknown tool: {}", self.0)
    }
}

impl IntoResponse for UnknownToolError {
    fn into_response(self) -> Response {
        (StatusCode::NOT_FOUND, self.to_string()).into_response()
    }
}

impl From<UnknownToolError> for McpError {
    fn from(error: UnknownToolError) -> Self {
        McpError::invalid_params(error.to_string(), None)
    }
}

/// Map [`ReadFileError`] onto an HTTP status.
impl IntoResponse for ReadFileError {
    fn into_response(self) -> Response {
        let status = match self {
            Self::RelativePath(_) | Self::ZeroLimit | Self::NotAFile(_) => StatusCode::BAD_REQUEST,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Io(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, self.to_string()).into_response()
    }
}

/// The JSON-RPC counterpart of the mapping above.
impl From<ReadFileError> for McpError {
    fn from(error: ReadFileError) -> Self {
        match &error {
            ReadFileError::RelativePath(_)
            | ReadFileError::ZeroLimit
            | ReadFileError::NotAFile(_) => McpError::invalid_params(error.to_string(), None),
            ReadFileError::NotFound(_) => McpError::resource_not_found(error.to_string(), None),
            ReadFileError::Io(_) => McpError::internal_error(error.to_string(), None),
        }
    }
}

// ---------------------------------------------------------------------------
// HTTP adapters
// ---------------------------------------------------------------------------

/// `GET /health` — liveness probe.
async fn http_health(State(state): State<Arc<AppState>>) -> Json<HealthResult> {
    Json(health_result(&state))
}

/// `GET /tools` — the same data as the MCP `list_tools` tool.
async fn http_list_tools() -> Json<ListToolsResult> {
    Json(all_tools())
}

/// `POST /tools/describe` — the same data as the MCP `describe_tool` tool.
async fn http_describe_tool(
    Json(params): Json<DescribeToolParams>,
) -> Result<Json<DescribeToolResult>, UnknownToolError> {
    find_tool(&params.name)
        .map(Json)
        .ok_or_else(|| UnknownToolError(params.name))
}

/// `POST /fs/readFile` — the same operation as the MCP `read_file` tool.
async fn http_read_file(
    Json(params): Json<ReadFileParams>,
) -> Result<Json<ReadFileResult>, ReadFileError> {
    Ok(Json(file::read_file(&params).await?))
}

/// Minimal request log, safe for MCP's long-lived SSE streams because it never
/// buffers the body.
async fn log_requests(request: Request, next: Next) -> Response {
    let method = request.method().clone();
    let uri = request.uri().clone();
    let response = next.run(request).await;
    tracing::info!(%method, %uri, status = %response.status(), "request");
    response
}

// ---------------------------------------------------------------------------
// MCP adapters
// ---------------------------------------------------------------------------

/// MCP handler, built once per session by the service factory.
#[derive(Clone)]
struct SandboxServer {
    state: Arc<AppState>,
}

#[tool_router]
impl SandboxServer {
    fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }

    #[tool(description = "Report server liveness and uptime")]
    fn health(&self) -> Result<CallToolResult, McpError> {
        structured(&health_result(&self.state))
    }

    #[tool(description = "List the command-line tools this server bundles")]
    fn list_tools(&self) -> Result<CallToolResult, McpError> {
        structured(&all_tools())
    }

    #[tool(description = "Describe a bundled command-line tool by name")]
    fn describe_tool(
        &self,
        Parameters(params): Parameters<DescribeToolParams>,
    ) -> Result<CallToolResult, McpError> {
        let result = find_tool(&params.name).ok_or(UnknownToolError(params.name))?;
        structured(&result)
    }

    #[tool(description = "Read a window of lines from a UTF-8 text file by absolute path")]
    async fn read_file(
        &self,
        Parameters(params): Parameters<ReadFileParams>,
    ) -> Result<CallToolResult, McpError> {
        let result = file::read_file(&params).await.map_err(McpError::from)?;
        structured(&result)
    }
}

/// Wrap a typed result as `structuredContent`.
fn structured<T: serde::Serialize>(value: &T) -> Result<CallToolResult, McpError> {
    let value = serde_json::to_value(value)
        .map_err(|error| McpError::internal_error(error.to_string(), None))?;
    Ok(CallToolResult::structured(value))
}

#[tool_handler]
impl ServerHandler for SandboxServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::from_build_env())
            .with_protocol_version(ProtocolVersion::V_2024_11_05)
            .with_instructions("Sandbox toolkit server".to_string())
    }
}
