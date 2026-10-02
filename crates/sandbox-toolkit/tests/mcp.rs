use std::collections::BTreeSet;
use std::fs::File;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use axum::Router;
use axum::routing::any_service;
use rmcp::ServerHandler;
use rmcp::ServiceExt;
use rmcp::model::{ErrorData, ListToolsResult, PaginatedRequestParams, ServerConfig, Tool};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{
    StreamableHttpClientTransport, StreamableHttpServerConfig, StreamableHttpService,
};
use serde_json::{Value, json};

const SCHEMA: &str = "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json";

/// A directory removed when the test ends.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let serial = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("sbxtkt-mcp-{tag}-{}-{serial}", std::process::id()));
        std::fs::create_dir_all(&path).expect("create temp dir");
        Self(std::fs::canonicalize(path).expect("canonicalize temp dir"))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The `sbxtkt` server under test, running on a private port with a throwaway root.
struct Server {
    child: Child,
    base_url: String,
    log: PathBuf,
}

impl Server {
    async fn start(home: &Path) -> Self {
        let port = free_port();
        let log = home.join("server.log");
        let child = Command::new(env!("CARGO_BIN_EXE_sbxtkt"))
            .args(["serve", "--host", "127.0.0.1", "--port", &port.to_string()])
            .env("HOME", home)
            .env("RUST_LOG", "warn")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(File::create(&log).expect("create server log")))
            .spawn()
            .expect("spawn sbxtkt");

        let mut server = Self {
            child,
            base_url: format!("http://127.0.0.1:{port}"),
            log,
        };
        server.wait_ready().await;
        server
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    async fn get(&self, path: &str) -> reqwest::Response {
        reqwest::get(self.url(path)).await.expect("GET request")
    }

    async fn post(&self, path: &str, body: Value) -> reqwest::Response {
        reqwest::Client::new()
            .post(self.url(path))
            .json(&body)
            .send()
            .await
            .expect("POST request")
    }

    /// The decoded JSON body of a `GET`, asserting it succeeded.
    async fn get_json(&self, path: &str) -> Value {
        let response = self.get(path).await;
        assert_eq!(response.status(), reqwest::StatusCode::OK, "GET {path}");
        response.json().await.expect("decode GET body")
    }

    /// The decoded `/mcps` document, the mount's primary observable.
    async fn list(&self) -> Value {
        let response = self.get("/mcps").await;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        response.json().await.expect("decode /mcps")
    }

    async fn wait_ready(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if reqwest::get(self.url("/mcps")).await.is_ok() {
                return;
            }
            if let Some(status) = self.child.try_wait().expect("poll server") {
                panic!(
                    "server exited with {status}: {}",
                    std::fs::read_to_string(&self.log).unwrap_or_default()
                );
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!(
            "server did not become ready: {}",
            std::fs::read_to_string(&self.log).unwrap_or_default()
        );
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn free_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .expect("reserve a port")
        .local_addr()
        .expect("read the reserved address")
        .port()
}

fn write_mcp_json(home: &Path, servers: Value) {
    let agents = home.join(".agents");
    std::fs::create_dir_all(&agents).expect("create .agents");
    let document = json!({ "$schema": SCHEMA, "mcpServers": servers });
    std::fs::write(agents.join("mcp.json"), document.to_string()).expect("write mcp.json");
}

fn ids(document: &Value) -> BTreeSet<String> {
    document["mcpServers"]
        .as_object()
        .map(|servers| servers.keys().cloned().collect())
        .unwrap_or_default()
}

/// Registers a workspace rooted at `root`, asserting the server accepted it.
async fn register(server: &Server, id: &str, root: &Path) {
    let body = json!({ "id": id, "root": root.to_string_lossy() });
    let response = server.post("/workspaces", body).await;
    assert_eq!(
        response.status(),
        reqwest::StatusCode::CREATED,
        "register workspace {id}"
    );
}

#[derive(Clone)]
struct Upstream {
    tools: Vec<Tool>,
}

#[allow(clippy::manual_async_fn)]
impl ServerHandler for Upstream {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::default()
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(self.tools.clone()))
    }
}

/// A minimal in-process MCP server exposing `names` as tools, for the proxy to
/// reverse-proxy to.
async fn start_upstream(names: &[&'static str]) -> String {
    let tools: Vec<Tool> = names
        .iter()
        .map(|name| Tool::new(*name, "", serde_json::Map::<String, Value>::new()))
        .collect();
    let service = StreamableHttpService::new(
        move || {
            Ok(Upstream {
                tools: tools.clone(),
            })
        },
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default().disable_allowed_hosts(),
    );
    let router = Router::new().route("/mcp", any_service(service));
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind the upstream");

    let address = listener.local_addr().expect("read the upstream address");
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });

    format!("http://{address}/mcp")
}

mod load {
    use super::*;

    /// Every entry in `.agents/mcp.json` shows up in `GET /mcps`.
    #[tokio::test]
    async fn discovers() {
        let home = TempDir::new("load");
        write_mcp_json(
            home.path(),
            json!({
                "validator": { "type": "stdio", "command": "./bin/validator" },
                "deployment-api": { "type": "streamable-http", "url": "http://127.0.0.1:9/mcp" },
            }),
        );
        let server = Server::start(home.path()).await;

        assert_eq!(
            ids(&server.list().await),
            BTreeSet::from(["deployment-api".to_owned(), "validator".to_owned()])
        );
    }

    /// Rewriting `.agents/mcp.json` changes what `GET /mcps` reports.
    #[tokio::test]
    async fn reloads() {
        let home = TempDir::new("reload");
        write_mcp_json(
            home.path(),
            json!({ "alpha": { "type": "stdio", "command": "alpha" } }),
        );
        let server = Server::start(home.path()).await;
        assert_eq!(
            ids(&server.list().await),
            BTreeSet::from(["alpha".to_owned()])
        );

        write_mcp_json(
            home.path(),
            json!({ "beta": { "type": "stdio", "command": "beta" } }),
        );

        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if ids(&server.list().await) == BTreeSet::from(["beta".to_owned()]) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the mount did not reload after mcp.json changed"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

mod proxy {
    use super::*;

    /// An unknown `mcp_id` answers 404.
    #[tokio::test]
    async fn not_found() {
        let home = TempDir::new("proxy-missing");
        write_mcp_json(home.path(), json!({}));
        let server = Server::start(home.path()).await;

        let response = reqwest::Client::new()
            .post(server.url("/mcps/missing"))
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .body(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}"#)
            .send()
            .await
            .expect("POST to an unknown entry");

        assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
    }

    /// A `streamable-http` entry proxies a handshake and a `list_tools` call to
    /// its upstream.
    #[tokio::test]
    async fn remote() {
        let upstream = start_upstream(&[]).await;
        let home = TempDir::new("proxy-remote");
        write_mcp_json(
            home.path(),
            json!({ "echo": { "type": "streamable-http", "url": upstream } }),
        );
        let server = Server::start(home.path()).await;

        let client = ()
            .serve(StreamableHttpClientTransport::from_uri(
                server.url("/mcps/echo"),
            ))
            .await
            .expect("initialize the proxy");

        let tools: ListToolsResult = client
            .peer()
            .list_tools(None)
            .await
            .expect("list tools through the proxy");
        assert!(tools.tools.is_empty());

        let _ = client.cancel().await;
    }

    /// A `stdio` entry's child is spawned with `PLUGIN_ROOT` and `PLUGIN_DATA`
    /// set.
    #[tokio::test]
    async fn stdio() {
        let home = TempDir::new("proxy-stdio");
        write_mcp_json(
            home.path(),
            json!({
                "env-probe": {
                    "type": "stdio",
                    "command": "sh",
                    "args": ["-c", "env > $PLUGIN_DATA/probe"],
                }
            }),
        );
        let server = Server::start(home.path()).await;

        // The child is not an MCP server, so the handshake fails; that the child
        // ran at all is what proves it was launched with the reserved variables.
        let _ = ()
            .serve(StreamableHttpClientTransport::from_uri(
                server.url("/mcps/env-probe"),
            ))
            .await;

        let probe = home.path().join(".agents/.data/env-probe/probe");
        let deadline = Instant::now() + Duration::from_secs(10);
        let env = loop {
            if let Ok(contents) = std::fs::read_to_string(&probe) {
                break contents;
            }
            assert!(Instant::now() < deadline, "the stdio child did not run");
            tokio::time::sleep(Duration::from_millis(50)).await;
        };

        assert!(env.contains("PLUGIN_ROOT="), "{env}");
        assert!(env.contains("PLUGIN_DATA="), "{env}");
    }
}

mod query {
    use super::*;

    /// `GET /mcps` presents every entry as `streamable-http` pointing at
    /// `/mcps/{mcp_id}`.
    #[tokio::test]
    async fn manifest() {
        let home = TempDir::new("query-shape");
        write_mcp_json(
            home.path(),
            json!({ "validator": { "type": "stdio", "command": "./bin/validator" } }),
        );
        let server = Server::start(home.path()).await;

        assert_eq!(
            server.list().await,
            json!({
                "$schema": SCHEMA,
                "mcpServers": {
                    "validator": {
                        "type": "streamable-http",
                        "url": server.url("/mcps/validator"),
                    }
                }
            })
        );
    }

    /// `GET /workspaces/global/mcps` addresses entries under the workspace
    /// prefix.
    #[tokio::test]
    async fn workspace() {
        let home = TempDir::new("query-workspace");
        write_mcp_json(
            home.path(),
            json!({ "validator": { "type": "stdio", "command": "./bin/validator" } }),
        );
        let server = Server::start(home.path()).await;

        let document: Value = server
            .get("/workspaces/global/mcps")
            .await
            .json()
            .await
            .expect("decode the workspace mount");

        assert_eq!(
            document["mcpServers"]["validator"]["url"],
            json!(server.url("/workspaces/global/mcps/validator"))
        );
    }

    /// An unknown workspace answers 404.
    #[tokio::test]
    async fn unknown_workspace() {
        let home = TempDir::new("query-missing");
        write_mcp_json(home.path(), json!({}));
        let server = Server::start(home.path()).await;

        let response = server.get("/workspaces/missing/mcps").await;
        assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
    }
}

mod merge {
    use super::*;

    /// A workspace's view adds global's entries to its own.
    #[tokio::test]
    async fn includes_global() {
        let home = TempDir::new("merge-includes");
        write_mcp_json(
            home.path(),
            json!({ "global-only": { "type": "stdio", "command": "global" } }),
        );
        let root = workspace_root(&home);
        write_mcp_json(
            &root,
            json!({ "workspace-only": { "type": "stdio", "command": "workspace" } }),
        );

        let server = Server::start(home.path()).await;
        register(&server, "ws", &root).await;

        assert_eq!(
            ids(&server.get_json("/workspaces/ws/mcps").await),
            BTreeSet::from(["global-only".to_owned(), "workspace-only".to_owned()])
        );
    }

    /// A name defined by both scopes resolves to the workspace's entry.
    #[tokio::test]
    async fn workspace_wins() {
        let global = start_upstream(&["global-tool"]).await;
        let local = start_upstream(&["workspace-tool"]).await;

        let home = TempDir::new("merge-wins");
        write_mcp_json(
            home.path(),
            json!({ "echo": { "type": "streamable-http", "url": global } }),
        );
        let root = workspace_root(&home);
        write_mcp_json(
            &root,
            json!({ "echo": { "type": "streamable-http", "url": local } }),
        );

        let server = Server::start(home.path()).await;
        register(&server, "ws", &root).await;

        let client = ()
            .serve(StreamableHttpClientTransport::from_uri(
                server.url("/workspaces/ws/mcps/echo"),
            ))
            .await
            .expect("initialize the proxy");

        let tools = client
            .peer()
            .list_tools(None)
            .await
            .expect("list tools through the proxy");
        let names: BTreeSet<String> = tools
            .tools
            .iter()
            .map(|tool| tool.name.to_string())
            .collect();
        assert_eq!(names, BTreeSet::from(["workspace-tool".to_owned()]));

        let _ = client.cancel().await;
    }

    /// A workspace without its own `.agents` still answers with global's entries.
    #[tokio::test]
    async fn absent_workspace() {
        let home = TempDir::new("merge-absent");
        write_mcp_json(
            home.path(),
            json!({ "global-only": { "type": "stdio", "command": "global" } }),
        );
        let root = workspace_root(&home);

        let server = Server::start(home.path()).await;
        register(&server, "ws", &root).await;

        assert_eq!(
            ids(&server.get_json("/workspaces/ws/mcps").await),
            BTreeSet::from(["global-only".to_owned()])
        );
    }

    /// A workspace root directory under `home`, created without `.agents`.
    fn workspace_root(home: &TempDir) -> PathBuf {
        let root = home.path().join("ws");
        std::fs::create_dir_all(&root).expect("create the workspace root");
        root
    }
}
