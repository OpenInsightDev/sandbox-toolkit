mod harness;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use harness::{Dir, Server, write_mcp_json};

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

}
