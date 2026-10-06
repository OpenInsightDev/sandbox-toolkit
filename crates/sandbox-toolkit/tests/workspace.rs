mod harness;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

use harness::{Dir, Server, collect, manifest, write_mcp_json, write_plugin};

const SKILL: &str = "---\nname: deploy\ndescription: Deploy.\n---\n\nShip it.\n";

fn write_skill(root: &Path) -> PathBuf {
    let directory = root.join(".agents/skills/deploy");
    std::fs::create_dir_all(&directory).expect("create .agents");
    std::fs::write(directory.join("SKILL.md"), SKILL).expect("write SKILL.md");
    directory
}

/// Creates `path` and returns its canonical absolute form, matching the `root`
/// the server reports.
fn dir(path: &Path) -> String {
    std::fs::create_dir_all(path).expect("create directory");
    std::fs::canonicalize(path)
        .expect("canonicalize directory")
        .to_string_lossy()
        .into_owned()
}

mod register {
    use super::*;

    #[tokio::test]
    async fn creates() {
        let home = Dir::new("register-creates");
        let server = Server::start(&home).await;
        let root = dir(&home.path().join("work"));

        let response = server
            .post("/workspaces", json!({ "id": "work", "root": root }))
            .await;
        assert_eq!(response.status(), reqwest::StatusCode::CREATED);
        let created: Value = response.json().await.expect("decode the created workspace");
        assert_eq!(created["id"], json!("work"));
        assert_eq!(created["root"], json!(root));
        assert_eq!(created["access"], json!("read-write"));

        assert_eq!(server.get_json("/workspaces/work").await, created);
    }

    #[tokio::test]
    async fn conflict() {
        let home = Dir::new("register-conflict");
        let server = Server::start(&home).await;
        let root = dir(&home.path().join("work"));
        let body = json!({ "id": "work", "root": root });

        assert_eq!(
            server.post("/workspaces", body.clone()).await.status(),
            reqwest::StatusCode::CREATED
        );
        assert_eq!(
            server.post("/workspaces", body).await.status(),
            reqwest::StatusCode::CONFLICT
        );
    }

    #[tokio::test]
    async fn invalid_root() {
        let home = Dir::new("register-invalid-root");
        let server = Server::start(&home).await;

        let relative = server
            .post(
                "/workspaces",
                json!({ "id": "relative", "root": "relative/path" }),
            )
            .await;
        assert_eq!(relative.status(), reqwest::StatusCode::BAD_REQUEST);

        let missing = home.path().join("missing");
        let absent = server
            .post(
                "/workspaces",
                json!({ "id": "absent", "root": missing.to_string_lossy() }),
            )
            .await;
        assert_eq!(absent.status(), reqwest::StatusCode::BAD_REQUEST);

        let file = home.path().join("file");
        std::fs::write(&file, "not a directory").expect("write file");
        let not_dir = server
            .post(
                "/workspaces",
                json!({ "id": "file", "root": file.to_string_lossy() }),
            )
            .await;
        assert_eq!(not_dir.status(), reqwest::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn invalid_id() {
        let home = Dir::new("register-invalid-id");
        let server = Server::start(&home).await;
        let root = dir(&home.path().join("work"));

        for id in ["", "a b", "team@eu"] {
            let response = server
                .post("/workspaces", json!({ "id": id, "root": root.clone() }))
                .await;
            assert_eq!(
                response.status(),
                reqwest::StatusCode::BAD_REQUEST,
                "id {id:?}"
            );
        }
    }

    #[tokio::test]
    async fn denied() {
        // Mode bits do not deny uid 0, so the probe cannot be refused that way.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }

        let home = Dir::new("register-denied");
        let server = Server::start(&home).await;

        use std::os::unix::fs::PermissionsExt;
        let root = home.path().join("readonly");
        std::fs::create_dir_all(&root).expect("create directory");
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o555))
            .expect("make the root read-only");

        let response = server
            .post(
                "/workspaces",
                json!({ "id": "readonly", "root": root.to_string_lossy() }),
            )
            .await;
        assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);

        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755))
            .expect("restore the root");
    }
}

mod query {
    use super::*;

    #[tokio::test]
    async fn list() {
        let home = Dir::new("query-list");
        let server = Server::start(&home).await;
        let root = dir(&home.path().join("work"));
        assert_eq!(
            server
                .post("/workspaces", json!({ "id": "work", "root": root }))
                .await
                .status(),
            reqwest::StatusCode::CREATED
        );

        let list = server.get_json("/workspaces").await;
        let ids: BTreeSet<String> = list
            .as_array()
            .expect("the workspace list is an array")
            .iter()
            .map(|workspace| {
                workspace["id"]
                    .as_str()
                    .expect("a workspace has an id")
                    .to_owned()
            })
            .collect();
        assert_eq!(
            ids,
            BTreeSet::from(["global".to_owned(), "work".to_owned()])
        );
    }

    #[tokio::test]
    async fn one() {
        let home = Dir::new("query-one");
        let server = Server::start(&home).await;
        let root = dir(&home.path().join("work"));
        server
            .post("/workspaces", json!({ "id": "work", "root": root }))
            .await;

        let workspace = server.get_json("/workspaces/work").await;
        assert_eq!(workspace["id"], json!("work"));
        assert_eq!(workspace["root"], json!(root));
    }

    #[tokio::test]
    async fn unknown() {
        let home = Dir::new("query-unknown");
        let server = Server::start(&home).await;

        let response = server.get("/workspaces/missing").await;
        assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
    }
}

mod update {
    use super::*;

    #[tokio::test]
    async fn access() {
        let home = Dir::new("update-access");
        let server = Server::start(&home).await;
        let root = dir(&home.path().join("work"));
        server
            .post("/workspaces", json!({ "id": "work", "root": root }))
            .await;

        let response = server
            .patch("/workspaces/work", json!({ "access": "read-only" }))
            .await;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let updated: Value = response.json().await.expect("decode the updated workspace");
        assert_eq!(updated["access"], json!("read-only"));

        assert_eq!(
            server.get_json("/workspaces/work").await["access"],
            json!("read-only")
        );
    }

    #[tokio::test]
    async fn unknown() {
        let home = Dir::new("update-unknown");
        let server = Server::start(&home).await;

        let response = server
            .patch("/workspaces/missing", json!({ "access": "read-only" }))
            .await;
        assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
    }
}

mod unregister {
    use super::*;

    #[tokio::test]
    async fn removes() {
        let home = Dir::new("unregister-removes");
        let server = Server::start(&home).await;
        let root = dir(&home.path().join("work"));
        server
            .post("/workspaces", json!({ "id": "work", "root": root }))
            .await;

        let response = server.delete("/workspaces/work").await;
        assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);

        assert_eq!(
            server.get("/workspaces/work").await.status(),
            reqwest::StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn unknown() {
        let home = Dir::new("unregister-unknown");
        let server = Server::start(&home).await;

        let response = server.delete("/workspaces/missing").await;
        assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
    }
}

mod global {
    use super::*;

    #[tokio::test]
    async fn default() {
        let home = Dir::new("global-default");
        let server = Server::start(&home).await;

        let workspace = server.get_json("/workspaces/global").await;
        assert_eq!(workspace["id"], json!("global"));
        assert_eq!(workspace["root"], json!(home.path().to_string_lossy()));
        assert_eq!(workspace["access"], json!("read-write"));
    }

    #[tokio::test]
    async fn immutable() {
        let home = Dir::new("global-immutable");
        let server = Server::start(&home).await;

        let patched = server
            .patch("/workspaces/global", json!({ "access": "read-only" }))
            .await;
        assert_eq!(patched.status(), reqwest::StatusCode::FORBIDDEN);

        let deleted = server.delete("/workspaces/global").await;
        assert_eq!(deleted.status(), reqwest::StatusCode::FORBIDDEN);
    }
}

mod resources {
    use super::*;

    #[tokio::test]
    async fn absent() {
        let home = Dir::new("agents-absent");
        let server = Server::start(&home).await;

        assert_eq!(
            server.get("/mcps").await.status(),
            reqwest::StatusCode::NOT_FOUND
        );
        assert_eq!(
            server.post("/mcps/missing", json!({})).await.status(),
            reqwest::StatusCode::NOT_FOUND
        );
        assert_eq!(
            server.get("/skills").await.status(),
            reqwest::StatusCode::NOT_FOUND
        );
        assert_eq!(
            server.get("/skills/missing").await.status(),
            reqwest::StatusCode::NOT_FOUND
        );
        assert_eq!(
            server.get("/plugins").await.status(),
            reqwest::StatusCode::NOT_FOUND
        );
        assert_eq!(
            server.get("/plugins/missing").await.status(),
            reqwest::StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn present() {
        let home = Dir::new("agents-present");
        write_mcp_json(home.path(), json!({}));
        let server = Server::start(&home).await;

        assert_eq!(server.get("/mcps").await.status(), reqwest::StatusCode::OK);

        // A scope that declares only skills holds resources all the same.
        let bare = Dir::new("agents-present-bare");
        write_skill(bare.path());
        let server = Server::start(&bare).await;

        assert_eq!(server.get("/mcps").await.status(), reqwest::StatusCode::OK);
        assert_eq!(
            server.get("/skills").await.status(),
            reqwest::StatusCode::OK
        );
    }

    #[tokio::test]
    async fn toggles() {
        let home = Dir::new("agents-toggles");
        let server = Server::start(&home).await;
        server.wait_status("/mcps", reqwest::StatusCode::NOT_FOUND).await;

        write_mcp_json(home.path(), json!({}));
        server.wait_status("/mcps", reqwest::StatusCode::OK).await;

        std::fs::remove_dir_all(home.path().join(".agents")).expect("remove .agents");
        server.wait_status("/mcps", reqwest::StatusCode::NOT_FOUND).await;
    }

    #[tokio::test]
    async fn broken() {
        let home = Dir::new("agents-broken");
        write_skill(home.path());
        std::fs::write(home.path().join(".agents/mcp.json"), "{ not json").expect("write mcp.json");

        let server = Server::start(&home).await;

        assert_eq!(
            server.get("/mcps").await.status(),
            reqwest::StatusCode::NOT_FOUND
        );
        assert_eq!(
            server.get("/skills").await.status(),
            reqwest::StatusCode::NOT_FOUND
        );
    }

    /// A plugin the loader rejects is not a partial loss: the whole resource set
    /// is absent, so every resource mount answers `404`.
    #[tokio::test]
    async fn broken_plugin() {
        let home = Dir::new("agents-broken-plugin");
        write_skill(home.path());
        write_mcp_json(home.path(), json!({}));
        write_plugin(home.path(), "broken", manifest("Deploy Kit"));

        let server = Server::start(&home).await;

        for path in ["/plugins", "/mcps", "/skills"] {
            assert_eq!(
                server.get(path).await.status(),
                reqwest::StatusCode::NOT_FOUND,
                "{path}"
            );
        }
    }
}

mod events {
    use super::*;

    /// How long each case listens before it stops expecting more events.
    const WINDOW: Duration = Duration::from_secs(3);

    #[tokio::test]
    async fn register() {
        let home = Dir::new("workspace-events-register");
        let server = Server::start(&home).await;
        let root = dir(&home.path().join("work"));

        let reported = server
            .events("/workspaces", WINDOW, async {
                let response = server
                    .post("/workspaces", json!({ "id": "work", "root": root }))
                    .await;
                assert_eq!(response.status(), reqwest::StatusCode::CREATED);
            })
            .await;

        assert!(
            reported.contains(&("register".to_owned(), json!({ "id": "work" }))),
            "a new workspace is registered, got {reported:?}"
        );
    }

    #[tokio::test]
    async fn unregister() {
        let home = Dir::new("workspace-events-unregister");
        let work = home.path().join("work");
        dir(&work);
        let server = Server::start(&home).await;
        server.register("work", &work).await;

        let reported = server
            .events("/workspaces", WINDOW, async {
                let response = server.delete("/workspaces/work").await;
                assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);
            })
            .await;

        assert!(
            reported.contains(&("unregister".to_owned(), json!({ "id": "work" }))),
            "a removed workspace is unregistered, got {reported:?}"
        );
    }

    #[tokio::test]
    async fn update() {
        let home = Dir::new("workspace-events-update");
        let work = home.path().join("work");
        dir(&work);
        let server = Server::start(&home).await;
        server.register("work", &work).await;

        let reported = server
            .events("/workspaces", WINDOW, async {
                let response = server
                    .patch("/workspaces/work", json!({ "access": "read-only" }))
                    .await;
                assert_eq!(response.status(), reqwest::StatusCode::OK);
            })
            .await;

        assert!(
            reported.contains(&("update".to_owned(), json!({ "id": "work" }))),
            "a changed workspace is updated, got {reported:?}"
        );
    }

    #[tokio::test]
    async fn negotiation() {
        let home = Dir::new("workspace-events-negotiation");
        let server = Server::start(&home).await;

        // Without the header the mount answers the array.
        assert!(server.get_json("/workspaces").await.is_array());

        let response = reqwest::Client::new()
            .get(server.url("/workspaces"))
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .send()
            .await
            .expect("open the events stream");
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok());
        assert_eq!(content_type, Some("text/event-stream"));
    }
}

mod summary {
    use super::*;

    /// How long each case listens before it stops expecting more events.
    const WINDOW: Duration = Duration::from_secs(3);

    #[tokio::test]
    async fn covers() {
        let home = Dir::new("summary-covers-home");
        let root = Dir::new("summary-covers-root");
        let agents = root.path().join(".agents");
        write_mcp_json(root.path(), json!({}));
        std::fs::create_dir_all(agents.join("skills")).expect("create the skills directory");
        std::fs::create_dir_all(agents.join("plugins")).expect("create the plugins directory");
        let server = Server::start(&home).await;
        server.register("ws", root.path()).await;

        let reported = server
            .events("/workspaces/ws", WINDOW, async {
                write_skill(root.path());
                write_plugin(root.path(), "deploy-kit", manifest("deploy-kit"));
                write_mcp_json(
                    root.path(),
                    json!({ "alpha": { "type": "stdio", "command": "alpha" } }),
                );
            })
            .await;

        for (resource, id) in [("skill", "deploy"), ("plugin", "deploy-kit"), ("mcp", "alpha")] {
            assert!(
                reported.contains(&("register".to_owned(), json!({ "resource": resource, "id": id }))),
                "the summary carries the {resource} event, got {reported:?}"
            );
        }
    }

    #[tokio::test]
    async fn shares() {
        let home = Dir::new("summary-shares-home");
        let root = Dir::new("summary-shares-root");
        std::fs::create_dir_all(root.path().join(".agents/skills"))
            .expect("create the skills directory");
        let server = Server::start(&home).await;
        server.register("ws", root.path()).await;

        // Both streams are open before the change, so each reports the same event.
        let mut summary = server.stream("/workspaces/ws").await;
        let mut skills = server.stream("/workspaces/ws/skills").await;

        write_skill(root.path());

        let from_summary = collect(&mut summary, WINDOW).await;
        let from_skills = collect(&mut skills, WINDOW).await;

        assert!(
            from_summary.contains(&(
                "register".to_owned(),
                json!({ "resource": "skill", "id": "deploy" }),
            )),
            "the summary reports the skill event, got {from_summary:?}"
        );
        assert!(
            from_skills.contains(&("register".to_owned(), json!({ "id": "deploy" }))),
            "the skill mount reports the same event, got {from_skills:?}"
        );
    }

    #[tokio::test]
    async fn excludes_registry() {
        let home = Dir::new("summary-excludes-home");
        let root = Dir::new("summary-excludes-root");
        write_mcp_json(root.path(), json!({}));
        let server = Server::start(&home).await;
        server.register("ws", root.path()).await;
        let other = dir(&home.path().join("other"));

        let reported = server
            .events("/workspaces/ws", WINDOW, async {
                // Another workspace's registration, and this one's access change,
                // are registry events rather than workspace content.
                let created = server
                    .post("/workspaces", json!({ "id": "other", "root": other }))
                    .await;
                assert_eq!(created.status(), reqwest::StatusCode::CREATED);
                let patched = server
                    .patch("/workspaces/ws", json!({ "access": "read-only" }))
                    .await;
                assert_eq!(patched.status(), reqwest::StatusCode::OK);
            })
            .await;

        assert!(
            reported.is_empty(),
            "registry events stay off the summary, got {reported:?}"
        );
    }

    #[tokio::test]
    async fn global() {
        let home = Dir::new("summary-global");
        std::fs::create_dir_all(home.path().join(".agents/skills"))
            .expect("create the skills directory");
        let server = Server::start(&home).await;

        let reported = server
            .events("/workspaces/global", WINDOW, async {
                write_skill(home.path());
            })
            .await;

        assert!(
            reported.contains(&(
                "register".to_owned(),
                json!({ "resource": "skill", "id": "deploy" }),
            )),
            "the global summary streams its own scope, got {reported:?}"
        );
    }

    #[tokio::test]
    async fn negotiation() {
        let home = Dir::new("summary-negotiation");
        let server = Server::start(&home).await;

        // Without the header the mount answers the workspace object.
        assert_eq!(
            server.get_json("/workspaces/global").await["id"],
            json!("global")
        );

        let response = server.stream("/workspaces/global").await;
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok());
        assert_eq!(content_type, Some("text/event-stream"));
    }
}
