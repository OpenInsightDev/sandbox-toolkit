mod harness;

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
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

use harness::{Dir, SCHEMA, Server, manifest, write_mcp_json, write_plugin, write_plugin_mcp};

fn ids(document: &Value) -> BTreeSet<String> {
    document["mcpServers"]
        .as_object()
        .map(|servers| servers.keys().cloned().collect())
        .unwrap_or_default()
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

    #[tokio::test]
    async fn discovers() {
        let home = Dir::new("load");
        write_mcp_json(
            home.path(),
            json!({
                "validator": { "type": "stdio", "command": "./bin/validator" },
                "deployment-api": { "type": "streamable-http", "url": "http://127.0.0.1:9/mcp" },
            }),
        );
        let server = Server::start(&home).await;

        assert_eq!(
            ids(&server.get_json("/mcps").await),
            BTreeSet::from(["deployment-api".to_owned(), "validator".to_owned()])
        );
    }

    /// A plugin the scope discovers contributes its `mcp.json` entries.
    #[tokio::test]
    async fn plugin() {
        let home = Dir::new("load-plugin");
        let plugin = write_plugin(home.path(), "deploy-kit", manifest("deploy-kit"));
        write_plugin_mcp(
            &plugin,
            json!({ "validator": { "type": "stdio", "command": "./bin/validator" } }),
        );
        let server = Server::start(&home).await;

        assert_eq!(
            ids(&server.get_json("/mcps").await),
            BTreeSet::from(["deploy-kit.validator".to_owned()])
        );
    }

    #[tokio::test]
    async fn reloads() {
        let home = Dir::new("reload");
        write_mcp_json(
            home.path(),
            json!({ "alpha": { "type": "stdio", "command": "alpha" } }),
        );
        let server = Server::start(&home).await;
        assert_eq!(
            ids(&server.get_json("/mcps").await),
            BTreeSet::from(["alpha".to_owned()])
        );

        write_mcp_json(
            home.path(),
            json!({ "beta": { "type": "stdio", "command": "beta" } }),
        );

        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if ids(&server.get_json("/mcps").await) == BTreeSet::from(["beta".to_owned()]) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the mount did not reload after mcp.json changed"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// A `mcp.json` that stops parsing after startup is not served from the last
    /// successful load: the mount reports the failure until it parses again, and
    /// only the MCP mount is affected.
    #[tokio::test]
    async fn broken() {
        let home = Dir::new("broken");
        write_mcp_json(
            home.path(),
            json!({ "validator": { "type": "stdio", "command": "./bin/validator" } }),
        );
        let server = Server::start(&home).await;
        assert_eq!(
            ids(&server.get_json("/mcps").await),
            BTreeSet::from(["validator".to_owned()])
        );

        std::fs::write(home.path().join(".agents/mcp.json"), "{ not json").expect("break mcp.json");

        server
            .wait_status("/mcps", reqwest::StatusCode::NOT_FOUND)
            .await;
        assert_eq!(
            server.get("/mcps/validator").await.status(),
            reqwest::StatusCode::NOT_FOUND
        );
        assert_eq!(
            server.get("/skills").await.status(),
            reqwest::StatusCode::OK
        );

        write_mcp_json(
            home.path(),
            json!({ "validator": { "type": "stdio", "command": "./bin/validator" } }),
        );
        server.wait_status("/mcps", reqwest::StatusCode::OK).await;
    }
}

mod proxy {
    use super::*;

    #[tokio::test]
    async fn not_found() {
        let home = Dir::new("proxy-missing");
        write_mcp_json(home.path(), json!({}));
        let server = Server::start(&home).await;

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

    #[tokio::test]
    async fn remote() {
        let upstream = start_upstream(&[]).await;
        let home = Dir::new("proxy-remote");
        write_mcp_json(
            home.path(),
            json!({ "echo": { "type": "streamable-http", "url": upstream } }),
        );
        let server = Server::start(&home).await;

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

    /// `.agents/mcp.json` is not a plugin, so its stdio entries are launched
    /// without the reserved plugin variables.
    #[tokio::test]
    async fn stdio() {
        let home = Dir::new("proxy-stdio");
        let probe = home.path().join("probe");
        write_mcp_json(
            home.path(),
            json!({
                "env-probe": {
                    "type": "stdio",
                    "command": "sh",
                    "args": ["-c", format!("env > {}", probe.display())],
                }
            }),
        );
        let server = Server::start(&home).await;

        // The child is not an MCP server, so the handshake fails; that the child
        // ran at all is what proves how it was launched.
        let _ = ()
            .serve(StreamableHttpClientTransport::from_uri(
                server.url("/mcps/env-probe"),
            ))
            .await;

        let deadline = Instant::now() + Duration::from_secs(10);
        let env = loop {
            if let Ok(contents) = std::fs::read_to_string(&probe) {
                break contents;
            }
            assert!(Instant::now() < deadline, "the stdio child did not run");
            tokio::time::sleep(Duration::from_millis(50)).await;
        };

        assert!(!env.contains("PLUGIN_ROOT="), "{env}");
        assert!(!env.contains("PLUGIN_DATA="), "{env}");
    }
}

mod query {
    use super::*;

    #[tokio::test]
    async fn manifest() {
        let home = Dir::new("query-shape");
        write_mcp_json(
            home.path(),
            json!({ "validator": { "type": "stdio", "command": "./bin/validator" } }),
        );
        let server = Server::start(&home).await;

        assert_eq!(
            server.get_json("/mcps").await,
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

    #[tokio::test]
    async fn workspace() {
        let home = Dir::new("query-workspace");
        write_mcp_json(
            home.path(),
            json!({ "validator": { "type": "stdio", "command": "./bin/validator" } }),
        );
        let server = Server::start(&home).await;

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

    #[tokio::test]
    async fn unknown_workspace() {
        let home = Dir::new("query-missing");
        write_mcp_json(home.path(), json!({}));
        let server = Server::start(&home).await;

        let response = server.get("/workspaces/missing/mcps").await;
        assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
    }
}

mod merge {
    use super::*;

    #[tokio::test]
    async fn includes_global() {
        let home = Dir::new("merge-includes");
        write_mcp_json(
            home.path(),
            json!({ "global-only": { "type": "stdio", "command": "global" } }),
        );
        let root = workspace_root(&home);
        write_mcp_json(
            &root,
            json!({ "workspace-only": { "type": "stdio", "command": "workspace" } }),
        );

        let server = Server::start(&home).await;
        server.register("ws", &root).await;

        assert_eq!(
            ids(&server.get_json("/workspaces/ws/mcps").await),
            BTreeSet::from(["global-only".to_owned(), "workspace-only".to_owned()])
        );
    }

    #[tokio::test]
    async fn workspace_wins() {
        let global = start_upstream(&["global-tool"]).await;
        let local = start_upstream(&["workspace-tool"]).await;

        let home = Dir::new("merge-wins");
        write_mcp_json(
            home.path(),
            json!({ "echo": { "type": "streamable-http", "url": global } }),
        );
        let root = workspace_root(&home);
        write_mcp_json(
            &root,
            json!({ "echo": { "type": "streamable-http", "url": local } }),
        );

        let server = Server::start(&home).await;
        server.register("ws", &root).await;

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

    /// A plugin's entry id carries the plugin id as a prefix, so the only way
    /// two entries collide is a same-named plugin in both scopes.
    #[tokio::test]
    async fn plugin_prefix() {
        let global = start_upstream(&["global-tool"]).await;
        let local = start_upstream(&["workspace-tool"]).await;

        let home = Dir::new("merge-plugin-prefix");
        let plugin = write_plugin(home.path(), "global", manifest("deploy-kit"));
        write_plugin_mcp(
            &plugin,
            json!({ "echo": { "type": "streamable-http", "url": global } }),
        );
        let root = workspace_root(&home);
        let plugin = write_plugin(&root, "local", manifest("deploy-kit"));
        write_plugin_mcp(
            &plugin,
            json!({ "echo": { "type": "streamable-http", "url": local } }),
        );

        let server = Server::start(&home).await;
        server.register("ws", &root).await;

        assert_eq!(
            ids(&server.get_json("/workspaces/ws/mcps").await),
            BTreeSet::from(["deploy-kit.echo".to_owned()])
        );

        let client = ()
            .serve(StreamableHttpClientTransport::from_uri(
                server.url("/workspaces/ws/mcps/deploy-kit.echo"),
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

    #[tokio::test]
    async fn absent_workspace() {
        let home = Dir::new("merge-absent");
        write_mcp_json(
            home.path(),
            json!({ "global-only": { "type": "stdio", "command": "global" } }),
        );
        let root = workspace_root(&home);

        let server = Server::start(&home).await;
        server.register("ws", &root).await;

        assert_eq!(
            ids(&server.get_json("/workspaces/ws/mcps").await),
            BTreeSet::from(["global-only".to_owned()])
        );
    }

    /// A scope that stops parsing its `mcp.json` takes the mount that merges it
    /// down with it, even while global's entries are intact.
    #[tokio::test]
    async fn broken() {
        let home = Dir::new("merge-broken");
        write_mcp_json(
            home.path(),
            json!({ "global-only": { "type": "stdio", "command": "global" } }),
        );
        let root = workspace_root(&home);
        write_mcp_json(
            &root,
            json!({ "workspace-only": { "type": "stdio", "command": "workspace" } }),
        );

        let server = Server::start(&home).await;
        server.register("ws", &root).await;
        assert_eq!(
            ids(&server.get_json("/workspaces/ws/mcps").await),
            BTreeSet::from(["global-only".to_owned(), "workspace-only".to_owned()])
        );

        std::fs::write(root.join(".agents/mcp.json"), "{ not json").expect("break mcp.json");

        server
            .wait_status("/workspaces/ws/mcps", reqwest::StatusCode::NOT_FOUND)
            .await;
    }

    fn workspace_root(home: &Dir) -> PathBuf {
        let root = home.path().join("ws");
        std::fs::create_dir_all(&root).expect("create the workspace root");
        root
    }
}
