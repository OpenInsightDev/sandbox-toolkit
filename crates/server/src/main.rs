//! `sandbox-toolkit` MCP server: the MCP Streamable HTTP endpoint at `/mcp`
//! plus ordinary HTTP routes over the same shared logic.

use std::{net::SocketAddr, sync::Arc, time::Instant};

use axum::{
    Json,
    body::Bytes,
    extract::{DefaultBodyLimit, Request, State},
    middleware::{self, Next},
    response::Response,
    routing::get,
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
use utoipa::OpenApi;
use utoipa_axum::{
    router::{OpenApiRouter, UtoipaMethodRouterExt},
    routes,
};

mod error;
mod file;
mod model;
mod plugins;
mod process;
mod tools;

use error::UnknownToolError;
use file::ReadFileError;
use model::{
    DescribeToolParams, DescribeToolResult, ExecParams, ExecResult, HealthResult, ListToolsResult,
    PluginParseResult, ReadFileParams, ReadFileResult,
};
use plugins::PluginError;
use process::ExecError;
use tools::Tool;

/// Path the MCP endpoint is mounted at.
const MCP_ENDPOINT: &str = "/mcp";

const OPENAPI_ENDPOINT: &str = "/api-docs/openapi.json";

#[derive(utoipa::OpenApi)]
#[openapi(
    info(
        title = "Sandbox Toolkit",
        description = "HTTP interface of the sandbox-toolkit server. The same operations are also exposed over MCP at `/mcp`.",
        version = env!("CARGO_PKG_VERSION")
    ),
    tags(
        (name = "system", description = "Liveness and server metadata."),
        (name = "tools", description = "The command-line tools bundled into this server."),
        (name = "filesystem", description = "Read windows of lines from text files."),
        (name = "process", description = "Run commands and capture their output."),
        (name = "plugins", description = "Parse uploaded Agent Plugins packages.")
    )
)]
struct ApiDoc;

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

    let (router, openapi) = openapi_router().split_for_parts();

    let router = router
        .nest_service(MCP_ENDPOINT, mcp)
        .route(
            OPENAPI_ENDPOINT,
            get(move || {
                let openapi = openapi.clone();
                async move { Json(openapi) }
            }),
        )
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

fn openapi_router() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::<Arc<AppState>>::with_openapi(ApiDoc::openapi())
        .routes(routes!(http_health))
        .routes(routes!(http_list_tools))
        .routes(routes!(http_describe_tool))
        .routes(routes!(http_read_file))
        .routes(routes!(http_exec))
        .routes(routes!(http_parse_plugin).layer(DefaultBodyLimit::max(plugins::MAX_ARCHIVE_BYTES)))
}

/// `GET /health` — liveness probe.
#[utoipa::path(
    get,
    path = "/health",
    tag = "system",
    responses((status = OK, description = "The server is running.", body = HealthResult))
)]
async fn http_health(State(state): State<Arc<AppState>>) -> Json<HealthResult> {
    Json(health_result(&state))
}

/// `GET /tools` — the same data as the MCP `list_tools` tool.
#[utoipa::path(
    get,
    path = "/tools",
    tag = "tools",
    responses((status = OK, description = "Every executable embedded in this build.", body = ListToolsResult))
)]
async fn http_list_tools() -> Json<ListToolsResult> {
    Json(all_tools())
}

/// `POST /tools/describe` — the same data as the MCP `describe_tool` tool.
#[utoipa::path(
    post,
    path = "/tools/describe",
    tag = "tools",
    request_body = DescribeToolParams,
    responses(
        (status = OK, description = "The described tool.", body = DescribeToolResult),
        (status = NOT_FOUND, description = "No bundled tool has that name.", body = String)
    )
)]
async fn http_describe_tool(
    Json(params): Json<DescribeToolParams>,
) -> Result<Json<DescribeToolResult>, UnknownToolError> {
    find_tool(&params.name)
        .map(Json)
        .ok_or(UnknownToolError(params.name))
}

/// `POST /fs/readFile` — the same operation as the MCP `read_file` tool.
#[utoipa::path(
    post,
    path = "/fs/readFile",
    tag = "filesystem",
    request_body = ReadFileParams,
    responses(
        (status = OK, description = "The lines in the requested window, in file order.", body = ReadFileResult),
        (status = BAD_REQUEST, description = "The path was not absolute or `limit` was zero.", body = String),
        (status = NOT_FOUND, description = "No file exists at the requested path.", body = String),
        (status = INTERNAL_SERVER_ERROR, description = "The file could not be read.", body = String)
    )
)]
async fn http_read_file(
    Json(params): Json<ReadFileParams>,
) -> Result<Json<ReadFileResult>, ReadFileError> {
    Ok(Json(file::read_file(&params).await?))
}

/// `POST /process/exec` — the same operation as the MCP `exec` tool.
#[utoipa::path(
    post,
    path = "/process/exec",
    tag = "process",
    request_body = ExecParams,
    responses(
        (status = OK, description = "The command's exit status and captured output.", body = ExecResult),
        (status = BAD_REQUEST, description = "The command, working directory or limit was invalid.", body = String),
        (status = INTERNAL_SERVER_ERROR, description = "The command output could not be captured.", body = String)
    )
)]
async fn http_exec(Json(params): Json<ExecParams>) -> Result<Json<ExecResult>, ExecError> {
    Ok(Json(process::exec(&params).await?))
}

/// `POST /plugins` — parse an uploaded Agent Plugins `.tar.gz`.
#[utoipa::path(
    post,
    path = "/plugins",
    tag = "plugins",
    request_body(
        content = Vec<u8>,
        content_type = "application/gzip",
        description = "A gzip-compressed tar archive holding the plugin package."
    ),
    responses(
        (status = OK, description = "The specification's verdict on the uploaded package.", body = PluginParseResult),
        (status = BAD_REQUEST, description = "The body was empty or not a usable tar archive.", body = String),
        (status = PAYLOAD_TOO_LARGE, description = "The archive exceeded the compressed size limit.", body = String),
        (status = INTERNAL_SERVER_ERROR, description = "Parsing the archive failed.", body = String)
    )
)]
async fn http_parse_plugin(body: Bytes) -> Result<Json<PluginParseResult>, PluginError> {
    Ok(Json(plugins::parse(body).await?))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openapi_document_describes_every_http_route() {
        let (_, openapi) = openapi_router().split_for_parts();

        for path in [
            "/health",
            "/tools",
            "/tools/describe",
            "/fs/readFile",
            "/process/exec",
            "/plugins",
        ] {
            assert!(
                openapi.paths.paths.contains_key(path),
                "the generated document does not describe {path}"
            );
        }
    }

    #[test]
    fn openapi_document_declares_every_wire_schema() {
        let (_, openapi) = openapi_router().split_for_parts();
        let schemas = openapi
            .components
            .expect("the document declares components")
            .schemas;

        for name in [
            "HealthResult",
            "ListToolsResult",
            "DescribeToolParams",
            "DescribeToolResult",
            "ReadFileParams",
            "ReadFileResult",
            "TextLine",
            "ExecParams",
            "ExecResult",
            "PluginParseResult",
            "PluginRejection",
            "PluginManifest",
            "PluginAuthor",
            "PluginSkill",
            "PluginMcp",
            "PluginMcpStatus",
            "PluginMcpServer",
            "PluginTransport",
            "PluginDiagnostic",
        ] {
            assert!(
                schemas.contains_key(name),
                "the generated document does not declare the {name} schema"
            );
        }
    }
}
