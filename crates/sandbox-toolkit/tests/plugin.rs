mod harness;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rmcp::ServiceExt;
use rmcp::transport::StreamableHttpClientTransport;
use serde_json::{Value, json};

use harness::{
    Dir, PLUGIN_SCHEMA, Server, canonical, manifest, plugins_dir, write_plugin, write_plugin_mcp,
    write_plugin_skill,
};

fn workspace_root(home: &Dir) -> PathBuf {
    let root = home.path().join("ws");
    std::fs::create_dir_all(&root).expect("create the workspace root");
    root
}

fn ids(document: &Value) -> Vec<String> {
    document
        .as_array()
        .expect("the plugin list is an array")
        .iter()
        .map(|entry| entry["id"].as_str().expect("plugin id").to_owned())
        .collect()
}

fn entry<'a>(document: &'a Value, id: &str) -> &'a Value {
    document
        .as_array()
        .expect("the plugin list is an array")
        .iter()
        .find(|entry| entry["id"].as_str() == Some(id))
        .unwrap_or_else(|| panic!("plugin `{id}` is missing"))
}

mod load {
    use super::*;

    #[tokio::test]
    async fn discovers() {
        let home = Dir::new("plugin-load-discovers");
        write_plugin(home.path(), "deploy-kit", manifest("deploy-kit"));
        write_plugin(home.path(), "lint-kit", manifest("lint-kit"));

        let server = Server::start(&home).await;

        assert_eq!(
            ids(&server.get_json("/plugins").await),
            ["deploy-kit", "lint-kit"]
        );
    }

    #[tokio::test]
    async fn id() {
        let home = Dir::new("plugin-load-id");
        let plugin = write_plugin(home.path(), "container", manifest("deploy-kit"));

        let server = Server::start(&home).await;

        assert_eq!(
            entry(&server.get_json("/plugins").await, "container"),
            &json!({
                "id": "container",
                "root": canonical(&plugin),
                "uri": "/plugins/container",
                "manifest": { "$schema": PLUGIN_SCHEMA, "name": "deploy-kit" },
            })
        );
    }

    #[tokio::test]
    async fn rescans() {
        let home = Dir::new("plugin-load-rescans");
        write_plugin(home.path(), "deploy-kit", manifest("deploy-kit"));

        let server = Server::start(&home).await;
        assert_eq!(ids(&server.get_json("/plugins").await), ["deploy-kit"]);

        write_plugin(home.path(), "lint-kit", manifest("lint-kit"));
        assert_eq!(
            ids(&server.get_json("/plugins").await),
            ["deploy-kit", "lint-kit"]
        );

        std::fs::remove_dir_all(plugins_dir(home.path()).join("deploy-kit"))
            .expect("remove a plugin");
        assert_eq!(ids(&server.get_json("/plugins").await), ["lint-kit"]);
    }

    /// A plugin the loader rejects takes the whole resource set with it, so
    /// every resource mount answers `404`.
    async fn rejects(home: &Dir) {
        let server = Server::start(home).await;

        for path in ["/plugins", "/mcps", "/skills"] {
            assert_eq!(
                server.get(path).await.status(),
                reqwest::StatusCode::NOT_FOUND,
                "{path}"
            );
        }
    }

    #[tokio::test]
    async fn rejected() {
        let home = Dir::new("plugin-load-rejected");
        let broken = plugins_dir(home.path()).join("broken");
        std::fs::create_dir_all(&broken).expect("create the plugin directory");
        std::fs::write(broken.join("plugin.json"), "{ not json").expect("write plugin.json");
        std::fs::create_dir_all(plugins_dir(home.path()).join("empty"))
            .expect("create a plugin directory without plugin.json");

        rejects(&home).await;
    }

    #[tokio::test]
    async fn disabled_mcp() {
        let home = Dir::new("plugin-load-disabled");
        let plugin = write_plugin(home.path(), "deploy-kit", manifest("deploy-kit"));
        std::fs::write(plugin.join("mcp.json"), "{ not json").expect("write mcp.json");

        rejects(&home).await;
    }

    #[tokio::test]
    async fn same_name() {
        let home = Dir::new("plugin-load-same-name");
        write_plugin(home.path(), "one", manifest("deploy-kit"));
        write_plugin(home.path(), "two", manifest("deploy-kit"));

        let server = Server::start(&home).await;

        assert_eq!(ids(&server.get_json("/plugins").await), ["one", "two"]);
    }

    /// Only a load failure fails the construction: everything the specification
    /// reports and ignores stays loaded.
    #[tokio::test]
    async fn ignored() {
        let home = Dir::new("plugin-load-ignored");
        let plugin = write_plugin(
            home.path(),
            "deploy-kit",
            json!({
                "$schema": PLUGIN_SCHEMA,
                "name": "deploy-kit",
                "unknown-field": true,
                "extensions": "not an object",
            }),
        );
        write_plugin_skill(&plugin, "good", "name: good\ndescription: Good.", "Good.\n");
        write_plugin_skill(&plugin, "broken", "description: No name.", "No.\n");
        write_plugin_mcp(&plugin, json!({ "validator": { "type": "stdio" } }));

        let server = Server::start(&home).await;

        assert_eq!(ids(&server.get_json("/plugins").await), ["deploy-kit"]);
    }
}

mod query {
    use super::*;

    #[tokio::test]
    async fn list() {
        let home = Dir::new("plugin-query-list");
        write_plugin(home.path(), "lint-kit", manifest("lint-kit"));
        write_plugin(home.path(), "deploy-kit", manifest("deploy-kit"));

        let server = Server::start(&home).await;

        assert_eq!(
            ids(&server.get_json("/plugins").await),
            ["deploy-kit", "lint-kit"]
        );
    }

    #[tokio::test]
    async fn pagination() {
        let home = Dir::new("plugin-query-pagination");
        for name in ["alpha", "beta", "gamma"] {
            write_plugin(home.path(), name, manifest(name));
        }

        let server = Server::start(&home).await;

        assert_eq!(
            ids(&server.get_json("/plugins?offset=1&limit=1").await),
            ["beta"]
        );
        assert_eq!(
            ids(&server.get_json("/plugins?limit=2").await),
            ["alpha", "beta"]
        );
        assert_eq!(
            ids(&server.get_json("/plugins").await),
            ["alpha", "beta", "gamma"]
        );
    }

    #[tokio::test]
    async fn fields() {
        let home = Dir::new("plugin-query-fields");
        let plugin = write_plugin(
            home.path(),
            "deploy-kit",
            json!({
                "$schema": PLUGIN_SCHEMA,
                "name": "deploy-kit",
                "version": "1.0.0",
                "description": "Deployment helpers.",
            }),
        );
        write_plugin_skill(&plugin, "good", "name: good\ndescription: Good.", "Good.\n");

        let server = Server::start(&home).await;

        assert_eq!(
            entry(&server.get_json("/plugins").await, "deploy-kit"),
            &json!({
                "id": "deploy-kit",
                "root": canonical(&plugin),
                "uri": "/plugins/deploy-kit",
                "manifest": {
                    "$schema": PLUGIN_SCHEMA,
                    "name": "deploy-kit",
                    "version": "1.0.0",
                    "description": "Deployment helpers.",
                },
            })
        );
    }

    #[tokio::test]
    async fn uri() {
        let home = Dir::new("plugin-query-uri");
        write_plugin(home.path(), "deploy-kit", manifest("deploy-kit"));
        let root = workspace_root(&home);
        write_plugin(&root, "deploy-kit", manifest("deploy-kit"));

        let server = Server::start(&home).await;
        server.register("ws", &root).await;

        assert_eq!(
            entry(&server.get_json("/plugins").await, "deploy-kit")["uri"],
            json!("/plugins/deploy-kit")
        );
        assert_eq!(
            entry(
                &server.get_json("/workspaces/ws/plugins").await,
                "deploy-kit"
            )["uri"],
            json!("/workspaces/ws/plugins/deploy-kit")
        );
    }

    #[tokio::test]
    async fn one() {
        let home = Dir::new("plugin-query-one");
        write_plugin(home.path(), "deploy-kit", manifest("deploy-kit"));

        let server = Server::start(&home).await;

        let document = server.get_json("/plugins").await;
        assert_eq!(
            server.get_json("/plugins/deploy-kit").await,
            entry(&document, "deploy-kit").clone()
        );
    }

    #[tokio::test]
    async fn unknown() {
        let home = Dir::new("plugin-query-unknown");
        write_plugin(home.path(), "deploy-kit", manifest("deploy-kit"));

        let server = Server::start(&home).await;

        assert_eq!(
            server.get("/plugins/missing").await.status(),
            reqwest::StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn workspace() {
        let home = Dir::new("plugin-query-workspace");
        let root = workspace_root(&home);
        write_plugin(&root, "deploy-kit", manifest("deploy-kit"));

        let server = Server::start(&home).await;
        server.register("ws", &root).await;

        assert_eq!(
            ids(&server.get_json("/workspaces/ws/plugins").await),
            ["deploy-kit"]
        );
    }

    #[tokio::test]
    async fn unknown_workspace() {
        let home = Dir::new("plugin-query-missing");
        write_plugin(home.path(), "deploy-kit", manifest("deploy-kit"));

        let server = Server::start(&home).await;

        assert_eq!(
            server.get("/workspaces/missing/plugins").await.status(),
            reqwest::StatusCode::NOT_FOUND
        );
    }
}

mod merge {
    use super::*;

    #[tokio::test]
    async fn includes_global() {
        let home = Dir::new("plugin-merge-includes");
        write_plugin(home.path(), "global-only", manifest("global-only"));
        let root = workspace_root(&home);
        write_plugin(&root, "workspace-only", manifest("workspace-only"));

        let server = Server::start(&home).await;
        server.register("ws", &root).await;

        assert_eq!(
            ids(&server.get_json("/workspaces/ws/plugins").await),
            ["global-only", "workspace-only"]
        );
    }

    #[tokio::test]
    async fn workspace_wins() {
        let home = Dir::new("plugin-merge-wins");
        write_plugin(
            home.path(),
            "deploy-kit",
            json!({
                "$schema": PLUGIN_SCHEMA,
                "name": "deploy-kit",
                "description": "Global.",
            }),
        );
        let root = workspace_root(&home);
        write_plugin(
            &root,
            "deploy-kit",
            json!({
                "$schema": PLUGIN_SCHEMA,
                "name": "deploy-kit",
                "description": "Workspace.",
            }),
        );

        let server = Server::start(&home).await;
        server.register("ws", &root).await;

        assert_eq!(
            server.get_json("/workspaces/ws/plugins/deploy-kit").await["manifest"]["description"],
            json!("Workspace.")
        );
    }

    #[tokio::test]
    async fn absent_workspace() {
        let home = Dir::new("plugin-merge-absent");
        write_plugin(home.path(), "global-only", manifest("global-only"));
        let root = workspace_root(&home);

        let server = Server::start(&home).await;
        server.register("ws", &root).await;

        assert_eq!(
            ids(&server.get_json("/workspaces/ws/plugins").await),
            ["global-only"]
        );
    }
}

mod variables {
    use super::*;

    #[tokio::test]
    async fn anchors() {
        let home = Dir::new("plugin-variables-anchors");
        let plugin = write_plugin(home.path(), "deploy-kit", manifest("deploy-kit"));
        write_plugin_mcp(
            &plugin,
            json!({
                "env-probe": {
                    "type": "stdio",
                    "command": "sh",
                    "args": ["-c", "env > $PLUGIN_DATA/probe"],
                }
            }),
        );

        let server = Server::start(&home).await;

        // The child is not an MCP server, so the handshake fails; that the child
        // ran at all is what proves it was launched with the reserved variables.
        let _ = ()
            .serve(StreamableHttpClientTransport::from_uri(
                server.url("/mcps/deploy-kit.env-probe"),
            ))
            .await;

        let probe = plugin.join(".data/probe");
        let deadline = Instant::now() + Duration::from_secs(10);
        let env = loop {
            if let Ok(contents) = std::fs::read_to_string(&probe) {
                break contents;
            }
            assert!(Instant::now() < deadline, "the stdio child did not run");
            tokio::time::sleep(Duration::from_millis(50)).await;
        };

        let root = canonical(&plugin);
        let data = canonical(&plugin.join(".data"));
        assert!(env.contains(&format!("PLUGIN_ROOT={root}")), "{env}");
        assert!(env.contains(&format!("PLUGIN_DATA={data}")), "{env}");
    }
}

mod extension {
    use super::*;

    const NAMESPACE: &str = "com.example.client";

    /// A plugin declaring `NAMESPACE`, with the directory that backs it.
    fn write_extended(root: &Path, dir: &str, name: &str) -> PathBuf {
        let plugin = write_plugin(
            root,
            dir,
            json!({
                "$schema": PLUGIN_SCHEMA,
                "name": name,
                "extensions": { (NAMESPACE): { "setting": true } },
            }),
        );
        std::fs::create_dir_all(plugin.join(NAMESPACE)).expect("create the extension directory");
        plugin
    }

    #[tokio::test]
    async fn resolves() {
        let home = Dir::new("plugin-extension-resolves");
        let plugin = write_extended(home.path(), "deploy-kit", "deploy-kit");

        let server = Server::start(&home).await;

        let id = format!("plugin.global.deploy-kit.{NAMESPACE}");
        let workspace = server.get_json(&format!("/workspaces/{id}")).await;
        assert_eq!(workspace["id"], json!(id));
        assert_eq!(workspace["root"], json!(canonical(&plugin.join(NAMESPACE))));
        assert_eq!(workspace["access"], json!("read-write"));
    }

    #[tokio::test]
    async fn not_listed() {
        let home = Dir::new("plugin-extension-not-listed");
        write_extended(home.path(), "deploy-kit", "deploy-kit");

        let server = Server::start(&home).await;

        let listed: Vec<String> = server
            .get_json("/workspaces")
            .await
            .as_array()
            .expect("the workspace list")
            .iter()
            .map(|workspace| workspace["id"].as_str().expect("a workspace id").to_owned())
            .collect();
        assert!(
            !listed.iter().any(|id| id.starts_with("plugin.")),
            "{listed:?}"
        );
    }

    #[tokio::test]
    async fn mutate_denied() {
        let home = Dir::new("plugin-extension-mutate");
        write_extended(home.path(), "deploy-kit", "deploy-kit");
        let id = format!("plugin.global.deploy-kit.{NAMESPACE}");

        let server = Server::start(&home).await;

        assert_eq!(
            server
                .patch(
                    &format!("/workspaces/{id}"),
                    json!({ "access": "read-only" })
                )
                .await
                .status(),
            reqwest::StatusCode::FORBIDDEN
        );
        assert_eq!(
            server.delete(&format!("/workspaces/{id}")).await.status(),
            reqwest::StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn unknown() {
        let home = Dir::new("plugin-extension-unknown");
        write_extended(home.path(), "deploy-kit", "deploy-kit");

        let server = Server::start(&home).await;

        let undeclared = "plugin.global.deploy-kit.com.example.other";
        assert_eq!(
            server.get(&format!("/workspaces/{undeclared}")).await.status(),
            reqwest::StatusCode::NOT_FOUND
        );

        let absent = format!("plugin.global.missing.{NAMESPACE}");
        assert_eq!(
            server.get(&format!("/workspaces/{absent}")).await.status(),
            reqwest::StatusCode::NOT_FOUND
        );
    }
}
