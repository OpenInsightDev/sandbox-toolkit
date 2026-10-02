use std::collections::HashMap;
use std::ffi::OsString;
use std::sync::Arc;

use agent_plugins::McpServer as PluginMcpServer;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CancelledNotificationParam, ClientRequest,
    CompleteRequest, CompleteRequestParams, CompleteResult, ErrorData, GetPromptRequestParams,
    GetPromptResponse, InitializeRequestParams, InitializeResult, ListPromptsResult,
    ListResourceTemplatesResult, ListResourcesResult, ListToolsResult, PaginatedRequestParams,
    ProgressNotificationParam, ReadResourceRequestParams, ReadResourceResponse, ServerResult,
};
use rmcp::service::{
    NotificationContext, Peer, RequestContext, RoleClient, RunningService, ServiceError,
};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{
    StreamableHttpClientTransport, StreamableHttpServerConfig, StreamableHttpService,
    TokioChildProcess,
};
use rmcp::{ServerHandler, ServiceExt};
use tokio::sync::OnceCell;
use tokio_util::sync::CancellationToken;

use super::config::Entry;

pub type Service = StreamableHttpService<ProxyHandler, LocalSessionManager>;

pub struct Proxy {
    pub service: Service,
    pub cancel: CancellationToken,
}

impl Proxy {
    pub fn new(entry: Entry, env: HashMap<String, OsString>) -> Self {
        let cancel = CancellationToken::new();
        let config = StreamableHttpServerConfig::default()
            .with_cancellation_token(cancel.clone())
            .disable_allowed_hosts();
        let service = StreamableHttpService::new(
            move || Ok(ProxyHandler::new(entry.clone(), env.clone())),
            Arc::new(LocalSessionManager::default()),
            config,
        );

        Self { service, cancel }
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

pub type Proxies = HashMap<String, Proxy>;

pub struct ProxyHandler {
    entry: Entry,
    env: HashMap<String, OsString>,
    upstream: Arc<OnceCell<RunningService<RoleClient, InitializeRequestParams>>>,
}

impl ProxyHandler {
    fn new(entry: Entry, env: HashMap<String, OsString>) -> Self {
        Self {
            entry,
            env,
            upstream: Arc::new(OnceCell::new()),
        }
    }

    async fn connect(&self, request: &InitializeRequestParams) -> Result<(), ErrorData> {
        self.upstream
            .get_or_try_init(|| async {
                let config = request.clone();
                match &self.entry.server {
                    PluginMcpServer::Stdio(stdio) => {
                        let plan = stdio.launch_plan(&self.entry.anchors);
                        tokio::fs::create_dir_all(&plan.plugin_data)
                            .await
                            .map_err(internal)?;
                        tokio::fs::create_dir_all(&plan.cwd)
                            .await
                            .map_err(internal)?;

                        let mut command = tokio::process::Command::from(plan.to_command());
                        command.envs(&self.env);
                        let transport = TokioChildProcess::new(command).map_err(internal)?;
                        config.serve(transport).await.map_err(client_error)
                    }
                    PluginMcpServer::StreamableHttp(remote) => {
                        let mut custom_headers = HashMap::new();
                        for (name, value) in &remote.headers {
                            let name = axum::http::HeaderName::from_bytes(name.as_bytes())
                                .map_err(internal)?;
                            let value =
                                axum::http::HeaderValue::from_str(value).map_err(internal)?;
                            custom_headers.insert(name, value);
                        }
                        let transport = StreamableHttpClientTransport::from_config(
                            StreamableHttpClientTransportConfig::with_uri(remote.url.as_str())
                                .custom_headers(custom_headers),
                        );
                        config.serve(transport).await.map_err(client_error)
                    }
                    _ => Err(internal("unsupported transport")),
                }
            })
            .await
            .map(|_| ())
    }

    async fn peer(&self) -> Result<&Peer<RoleClient>, ErrorData> {
        self.upstream
            .get()
            .map(RunningService::peer)
            .ok_or_else(|| ErrorData::internal_error("upstream is not connected", None))
    }

    async fn forward_client_request(
        &self,
        request: ClientRequest,
    ) -> Result<ServerResult, ErrorData> {
        self.peer()
            .await?
            .send_request(request)
            .await
            .map_err(service_error)
    }
}

#[allow(clippy::manual_async_fn)]
impl ServerHandler for ProxyHandler {
    async fn initialize(
        &self,
        request: InitializeRequestParams,
        _context: RequestContext<rmcp::RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        self.connect(&request).await?;
        let running = self
            .upstream
            .get()
            .ok_or_else(|| ErrorData::internal_error("upstream is not connected", None))?;
        let info = running
            .peer_info()
            .ok_or_else(|| ErrorData::internal_error("upstream did not report its info", None))?;

        let result = InitializeResult::new(info.capabilities.clone())
            .with_protocol_version(info.protocol_version.clone())
            .with_server_info(
                info.server_info
                    .clone()
                    .unwrap_or_else(rmcp::model::Implementation::from_build_env),
            );

        Ok(match &info.instructions {
            Some(instructions) => result.with_instructions(instructions.clone()),
            None => result,
        })
    }

    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<rmcp::RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        self.peer()
            .await?
            .list_tools(request)
            .await
            .map_err(service_error)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<rmcp::RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        self.peer()
            .await?
            .call_tool_once(request)
            .await
            .map_err(service_error)
    }

    async fn list_prompts(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<rmcp::RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        self.peer()
            .await?
            .list_prompts(request)
            .await
            .map_err(service_error)
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<rmcp::RoleServer>,
    ) -> Result<GetPromptResponse, ErrorData> {
        self.peer()
            .await?
            .get_prompt_once(request)
            .await
            .map_err(service_error)
    }

    async fn list_resources(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<rmcp::RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        self.peer()
            .await?
            .list_resources(request)
            .await
            .map_err(service_error)
    }

    async fn list_resource_templates(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<rmcp::RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        self.peer()
            .await?
            .list_resource_templates(request)
            .await
            .map_err(service_error)
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<rmcp::RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        self.peer()
            .await?
            .read_resource_once(request)
            .await
            .map_err(service_error)
    }

    async fn complete(
        &self,
        request: CompleteRequestParams,
        _context: RequestContext<rmcp::RoleServer>,
    ) -> Result<CompleteResult, ErrorData> {
        match self
            .forward_client_request(ClientRequest::CompleteRequest(CompleteRequest::new(
                request,
            )))
            .await?
        {
            ServerResult::CompleteResult(result) => Ok(result),
            _ => Err(unexpected()),
        }
    }

    async fn on_cancelled(
        &self,
        notification: CancelledNotificationParam,
        _context: NotificationContext<rmcp::RoleServer>,
    ) {
        if let Ok(peer) = self.peer().await {
            let _ = peer.notify_cancelled(notification).await;
        }
    }

    async fn on_progress(
        &self,
        notification: ProgressNotificationParam,
        _context: NotificationContext<rmcp::RoleServer>,
    ) {
        if let Ok(peer) = self.peer().await {
            let _ = peer.notify_progress(notification).await;
        }
    }
}

fn internal(error: impl std::fmt::Display) -> ErrorData {
    ErrorData::internal_error(error.to_string(), None)
}

fn client_error(error: rmcp::service::ClientInitializeError) -> ErrorData {
    internal(error)
}

fn service_error(error: ServiceError) -> ErrorData {
    match error {
        ServiceError::McpError(data) => data,
        other => internal(other),
    }
}

fn unexpected() -> ErrorData {
    ErrorData::internal_error("unexpected upstream response", None)
}
