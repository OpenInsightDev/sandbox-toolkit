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
mod fs;
mod model;
mod plugins;
mod process;
mod tools;

use error::UnknownToolError;
use fs::FsError;
use model::{
    CopyParams, CopyResult, DescribeToolParams, DescribeToolResult, ExecParams, ExecResult,
    HealthResult, ListParams, ListResult, ListToolsResult, MkdirParams, MkdirResult, MoveParams,
    MoveResult, PluginParseResult, ReadFileParams, ReadFileResult, RemoveParams, RemoveResult,
    StatParams, StatResult, WriteFileParams, WriteFileResult,
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
        (name = "filesystem", description = "Read, describe, list, create, write, copy, move and remove files and directories."),
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

    let tools_dir = tools::materialize()
        .await
        .wrap_err("failed to materialize the bundled tools")?;
    tracing::info!(dir = %tools_dir.display(), "materialized the bundled tools");

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
        .routes(routes!(http_stat))
        .routes(routes!(http_list))
        .routes(routes!(http_mkdir))
        .routes(routes!(http_write_file))
        .routes(routes!(http_remove))
        .routes(routes!(http_copy))
        .routes(routes!(http_move))
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
) -> Result<Json<ReadFileResult>, FsError> {
    Ok(Json(fs::read_file(&params).await?))
}

/// `POST /fs/stat` — the same operation as the MCP `stat` tool.
#[utoipa::path(
    post,
    path = "/fs/stat",
    tag = "filesystem",
    request_body = StatParams,
    responses(
        (status = OK, description = "Metadata for the described entry.", body = StatResult),
        (status = BAD_REQUEST, description = "The path was not absolute or named a kind of entry that cannot be described.", body = String),
        (status = NOT_FOUND, description = "No entry exists at the requested path.", body = String),
        (status = INTERNAL_SERVER_ERROR, description = "The entry could not be described.", body = String)
    )
)]
async fn http_stat(Json(params): Json<StatParams>) -> Result<Json<StatResult>, FsError> {
    Ok(Json(fs::stat(&params).await?))
}

/// `POST /fs/list` — the same operation as the MCP `list` tool.
#[utoipa::path(
    post,
    path = "/fs/list",
    tag = "filesystem",
    request_body = ListParams,
    responses(
        (status = OK, description = "The directory's immediate members, sorted by name.", body = ListResult),
        (status = BAD_REQUEST, description = "The path was not absolute or was not a directory.", body = String),
        (status = NOT_FOUND, description = "No entry exists at the requested path.", body = String),
        (status = INTERNAL_SERVER_ERROR, description = "The directory could not be read.", body = String)
    )
)]
async fn http_list(Json(params): Json<ListParams>) -> Result<Json<ListResult>, FsError> {
    Ok(Json(fs::list(&params).await?))
}

/// `POST /fs/mkdir` — the same operation as the MCP `mkdir` tool.
#[utoipa::path(
    post,
    path = "/fs/mkdir",
    tag = "filesystem",
    request_body = MkdirParams,
    responses(
        (status = OK, description = "Metadata for the collection that was created.", body = MkdirResult),
        (status = BAD_REQUEST, description = "The path was not absolute, already existed, or its parents were missing.", body = String),
        (status = INTERNAL_SERVER_ERROR, description = "The directory could not be created.", body = String)
    )
)]
async fn http_mkdir(Json(params): Json<MkdirParams>) -> Result<Json<MkdirResult>, FsError> {
    Ok(Json(fs::mkdir(&params).await?))
}

/// `POST /fs/writeFile` — the same operation as the MCP `write_file` tool.
#[utoipa::path(
    post,
    path = "/fs/writeFile",
    tag = "filesystem",
    request_body = WriteFileParams,
    responses(
        (status = OK, description = "Metadata for the file that was written.", body = WriteFileResult),
        (status = BAD_REQUEST, description = "The path was not absolute or names a directory.", body = String),
        (status = INTERNAL_SERVER_ERROR, description = "The file could not be written.", body = String)
    )
)]
async fn http_write_file(
    Json(params): Json<WriteFileParams>,
) -> Result<Json<WriteFileResult>, FsError> {
    Ok(Json(fs::write_file(&params).await?))
}

/// `POST /fs/remove` — the same operation as the MCP `remove` tool.
#[utoipa::path(
    post,
    path = "/fs/remove",
    tag = "filesystem",
    request_body = RemoveParams,
    responses(
        (status = OK, description = "The path that was removed.", body = RemoveResult),
        (status = BAD_REQUEST, description = "The path was not absolute or the directory was not empty.", body = String),
        (status = NOT_FOUND, description = "No entry exists at the requested path.", body = String),
        (status = INTERNAL_SERVER_ERROR, description = "The entry could not be removed.", body = String)
    )
)]
async fn http_remove(Json(params): Json<RemoveParams>) -> Result<Json<RemoveResult>, FsError> {
    Ok(Json(fs::remove(&params).await?))
}

/// `POST /fs/copy` — the same operation as the MCP `copy` tool.
#[utoipa::path(
    post,
    path = "/fs/copy",
    tag = "filesystem",
    request_body = CopyParams,
    responses(
        (status = OK, description = "Metadata for the copy that was created.", body = CopyResult),
        (status = BAD_REQUEST, description = "A path was not absolute, the destination existed, or the paths overlapped.", body = String),
        (status = NOT_FOUND, description = "No entry exists at the requested source.", body = String),
        (status = INTERNAL_SERVER_ERROR, description = "The entry could not be copied.", body = String)
    )
)]
async fn http_copy(Json(params): Json<CopyParams>) -> Result<Json<CopyResult>, FsError> {
    Ok(Json(fs::copy(&params).await?))
}

/// `POST /fs/move` — the same operation as the MCP `move` tool.
#[utoipa::path(
    post,
    path = "/fs/move",
    tag = "filesystem",
    request_body = MoveParams,
    responses(
        (status = OK, description = "Metadata for the entry at its new location.", body = MoveResult),
        (status = BAD_REQUEST, description = "A path was not absolute, the destination existed, or the paths overlapped.", body = String),
        (status = NOT_FOUND, description = "No entry exists at the requested source.", body = String),
        (status = INTERNAL_SERVER_ERROR, description = "The entry could not be moved.", body = String)
    )
)]
async fn http_move(Json(params): Json<MoveParams>) -> Result<Json<MoveResult>, FsError> {
    Ok(Json(fs::move_(&params).await?))
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
        let result = fs::read_file(&params).await.map_err(McpError::from)?;
        structured(&result)
    }

    #[tool(description = "Describe one file, directory or symbolic link by absolute path")]
    async fn stat(
        &self,
        Parameters(params): Parameters<StatParams>,
    ) -> Result<CallToolResult, McpError> {
        let result = fs::stat(&params).await.map_err(McpError::from)?;
        structured(&result)
    }

    #[tool(description = "List a directory's immediate members by absolute path")]
    async fn list(
        &self,
        Parameters(params): Parameters<ListParams>,
    ) -> Result<CallToolResult, McpError> {
        let result = fs::list(&params).await.map_err(McpError::from)?;
        structured(&result)
    }

    #[tool(description = "Create a directory, optionally creating missing parents")]
    async fn mkdir(
        &self,
        Parameters(params): Parameters<MkdirParams>,
    ) -> Result<CallToolResult, McpError> {
        let result = fs::mkdir(&params).await.map_err(McpError::from)?;
        structured(&result)
    }

    #[tool(description = "Create or replace a text file, optionally appending instead")]
    async fn write_file(
        &self,
        Parameters(params): Parameters<WriteFileParams>,
    ) -> Result<CallToolResult, McpError> {
        let result = fs::write_file(&params).await.map_err(McpError::from)?;
        structured(&result)
    }

    #[tool(description = "Remove a file, symbolic link or directory by absolute path")]
    async fn remove(
        &self,
        Parameters(params): Parameters<RemoveParams>,
    ) -> Result<CallToolResult, McpError> {
        let result = fs::remove(&params).await.map_err(McpError::from)?;
        structured(&result)
    }

    #[tool(description = "Copy a file, symbolic link or directory to an absolute destination")]
    async fn copy(
        &self,
        Parameters(params): Parameters<CopyParams>,
    ) -> Result<CallToolResult, McpError> {
        let result = fs::copy(&params).await.map_err(McpError::from)?;
        structured(&result)
    }

    #[tool(
        name = "move",
        description = "Move or rename a file, symbolic link or directory"
    )]
    async fn move_entry(
        &self,
        Parameters(params): Parameters<MoveParams>,
    ) -> Result<CallToolResult, McpError> {
        let result = fs::move_(&params).await.map_err(McpError::from)?;
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
            "/fs/stat",
            "/fs/list",
            "/fs/mkdir",
            "/fs/writeFile",
            "/fs/remove",
            "/fs/copy",
            "/fs/move",
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
            "ResourceKind",
            "Resource",
            "StatParams",
            "StatResult",
            "ListParams",
            "ListResult",
            "MkdirParams",
            "MkdirResult",
            "WriteFileParams",
            "WriteFileResult",
            "RemoveParams",
            "RemoveResult",
            "CopyParams",
            "CopyResult",
            "MoveParams",
            "MoveResult",
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
