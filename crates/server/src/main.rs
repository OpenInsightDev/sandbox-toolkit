//! `sandbox-toolkit` MCP server: the MCP Streamable HTTP endpoint at `/mcp`
//! plus ordinary HTTP routes over the same shared logic.

use std::{net::SocketAddr, sync::Arc, time::Instant};

use axum::{
    Json, Router,
    extract::{Request, State},
    middleware::{self, Next},
    response::Response,
    routing::{get, post},
};
use clap::Parser;
use eyre::WrapErr;
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

mod error;
mod file;
mod model;
mod process;
mod tools;

use error::UnknownToolError;
use file::ReadFileError;
use model::{
    DescribeToolParams, DescribeToolResult, ExecParams, ExecResult, HealthResult, ListToolsResult,
    ReadFileParams, ReadFileResult,
};
use process::ExecError;
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
async fn main() -> eyre::Result<()> {
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
        .route("/process/exec", post(http_exec))
        .layer(middleware::from_fn(log_requests))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(args.bind)
        .await
        .wrap_err_with(|| format!("failed to bind {}", args.bind))?;
    tracing::info!(bind = %args.bind, mcp = MCP_ENDPOINT, "server listening");

    axum::serve(listener, router)
        .with_graceful_shutdown({
            let cancellation_token = cancellation_token.clone();
            async move {
                let _ = tokio::signal::ctrl_c().await;
                cancellation_token.cancel();
            }
        })
        .await
        .wrap_err("server stopped with an error")?;

    Ok(())
}

impl From<Tool> for DescribeToolResult {
    fn from(tool: Tool) -> Self {
        Self {
            name: tool.name.to_string(),
            path: tools::materialized_dir()
                .join(tool.name)
                .to_string_lossy()
                .into_owned(),
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
        .ok_or(UnknownToolError(params.name))
}

/// `POST /fs/readFile` — the same operation as the MCP `read_file` tool.
async fn http_read_file(
    Json(params): Json<ReadFileParams>,
) -> Result<Json<ReadFileResult>, ReadFileError> {
    Ok(Json(file::read_file(&params).await?))
}

/// `POST /process/exec` — the same operation as the MCP `exec` tool.
async fn http_exec(Json(params): Json<ExecParams>) -> Result<Json<ExecResult>, ExecError> {
    Ok(Json(process::exec(&params).await?))
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

    #[tool(description = "Run a command and capture its exit status and output")]
    async fn exec(
        &self,
        Parameters(params): Parameters<ExecParams>,
    ) -> Result<CallToolResult, McpError> {
        let result = process::exec(&params).await.map_err(McpError::from)?;
        structured(&result)
    }
}

/// Wrap a typed result as `structuredContent`.
fn structured<T: serde::Serialize>(value: &T) -> Result<CallToolResult, McpError> {
    let value = serde_json::to_value(value).map_err(|error| {
        McpError::internal_error(format!("failed to serialize tool result: {error}"), None)
    })?;
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
