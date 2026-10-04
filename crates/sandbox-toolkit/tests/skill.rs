use std::collections::BTreeSet;
use std::fs::File;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// A directory removed when the test ends.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let serial = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "sbxtkt-skill-{tag}-{}-{serial}",
            std::process::id()
        ));
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

/// The `sbxtkt` server under test, running on a private port with a throwaway home.
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

    async fn patch(&self, path: &str, body: Value) -> reqwest::Response {
        reqwest::Client::new()
            .patch(self.url(path))
            .json(&body)
            .send()
            .await
            .expect("PATCH request")
    }

    async fn delete(&self, path: &str) -> reqwest::Response {
        reqwest::Client::new()
            .delete(self.url(path))
            .send()
            .await
            .expect("DELETE request")
    }

    /// The decoded JSON body of a `GET`, asserting it succeeded.
    async fn get_json(&self, path: &str) -> Value {
        let response = self.get(path).await;
        assert_eq!(response.status(), reqwest::StatusCode::OK, "GET {path}");
        response.json().await.expect("decode GET body")
    }

    async fn wait_ready(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if reqwest::get(self.url("/workspaces")).await.is_ok() {
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

/// The scope's discovery directory, as the service addresses it.
fn skills_dir(root: &Path) -> PathBuf {
    root.join(".agents/skills")
}

/// Writes `<skills>/<id>/SKILL.md`, returning the skill directory.
fn write_skill(skills: &Path, id: &str, frontmatter: &str, body: &str) -> PathBuf {
    let dir = skills.join(id);
    std::fs::create_dir_all(&dir).expect("create the skill directory");
    let document = format!("---\n{frontmatter}\n---\n\n{body}");
    std::fs::write(dir.join("SKILL.md"), document).expect("write SKILL.md");
    dir
}

/// Registers a workspace rooted at `root`, asserting the server accepted it.
async fn register(server: &Server, id: &str, root: &Path) {
    register_as(server, id, root, "read-write").await;
}

/// Registers with an explicit `access`, which a derived workspace inherits.
async fn register_as(server: &Server, id: &str, root: &Path, access: &str) {
    let body = json!({ "id": id, "root": root.to_string_lossy(), "access": access });
    let response = server.post("/workspaces", body).await;
    assert_eq!(
        response.status(),
        reqwest::StatusCode::CREATED,
        "register workspace {id}"
    );
}

/// The skill ids a `/skills` document carries, in order.
fn ids(document: &Value) -> Vec<String> {
    document["skills"]
        .as_array()
        .expect("the skills array")
        .iter()
        .map(|entry| entry["id"].as_str().expect("skill id").to_owned())
        .collect()
}

/// The entry of `id`, asserting the document carries it.
fn entry<'a>(document: &'a Value, id: &str) -> &'a Value {
    document["skills"]
        .as_array()
        .expect("the skills array")
        .iter()
        .find(|entry| entry["id"].as_str() == Some(id))
        .unwrap_or_else(|| panic!("skill `{id}` is missing"))
}

mod load {
    use super::*;

    /// Every valid child directory of `.agents/skills` is a skill named after it.
    #[tokio::test]
    async fn discovers() {
        let home = TempDir::new("load-discovers");
        let skills = skills_dir(home.path());
        write_skill(
            &skills,
            "deploy",
            "name: deploy\ndescription: Roll out a service.",
            "Ship it.\n",
        );
        write_skill(
            &skills,
            "other",
            "name: other\ndescription: Another skill.",
            "Other.\n",
        );

        let server = Server::start(home.path()).await;

        assert_eq!(ids(&server.get_json("/skills").await), ["deploy", "other"]);
    }

    /// A child directory the specification rejects is skipped rather than reported.
    #[tokio::test]
    async fn skips() {
        let home = TempDir::new("load-skips");
        let skills = skills_dir(home.path());
        write_skill(
            &skills,
            "deploy",
            "name: deploy\ndescription: Roll out a service.",
            "Ship it.\n",
        );

        std::fs::create_dir_all(skills.join("empty")).expect("create a directory without SKILL.md");
        write_skill(
            &skills,
            "mismatch",
            "name: different\ndescription: No.",
            "No.",
        );
        write_skill(&skills, "broken", "description: No name.", "No.");
        write_skill(
            &skills.join("nested"),
            "deep",
            "name: deep\ndescription: Deeper than a skill.",
            "Deep.",
        );
        std::fs::write(skills.join("loose.txt"), "not a skill").expect("write a loose file");

        let server = Server::start(home.path()).await;

        assert_eq!(ids(&server.get_json("/skills").await), ["deploy"]);
    }

    /// Discovery runs per request, so a change on disk shows up in the next answer.
    #[tokio::test]
    async fn rescans() {
        let home = TempDir::new("load-rescans");
        let skills = skills_dir(home.path());
        write_skill(
            &skills,
            "deploy",
            "name: deploy\ndescription: Roll out a service.",
            "Ship it.\n",
        );

        let server = Server::start(home.path()).await;

        write_skill(
            &skills,
            "gamma",
            "name: gamma\ndescription: Added later.",
            "Late.",
        );
        assert_eq!(ids(&server.get_json("/skills").await), ["deploy", "gamma"]);

        std::fs::remove_dir_all(skills.join("deploy")).expect("remove a skill");
        assert_eq!(ids(&server.get_json("/skills").await), ["gamma"]);
    }
}

mod merge {
    use super::*;

    /// A workspace's view adds global's skills to its own.
    #[tokio::test]
    async fn includes_global() {
        let home = TempDir::new("merge-includes-home");
        let root = TempDir::new("merge-includes-root");
        write_skill(
            &skills_dir(home.path()),
            "global-only",
            "name: global-only\ndescription: Discovered under the home directory.",
            "Global.\n",
        );
        write_skill(
            &skills_dir(root.path()),
            "workspace-only",
            "name: workspace-only\ndescription: Registered with the workspace.",
            "Workspace.\n",
        );

        let server = Server::start(home.path()).await;
        register(&server, "docs", root.path()).await;

        assert_eq!(
            ids(&server.get_json("/workspaces/docs/skills").await),
            ["global-only", "workspace-only"]
        );
    }

    /// A name defined by both scopes resolves to the workspace's skill.
    #[tokio::test]
    async fn workspace_wins() {
        let home = TempDir::new("merge-wins-home");
        let root = TempDir::new("merge-wins-root");
        write_skill(
            &skills_dir(home.path()),
            "deploy",
            "name: deploy\ndescription: Global deploy.",
            "Global body.\n",
        );
        write_skill(
            &skills_dir(root.path()),
            "deploy",
            "name: deploy\ndescription: Workspace deploy.",
            "Workspace body.\n",
        );

        let server = Server::start(home.path()).await;
        register(&server, "docs", root.path()).await;

        let response = server.get("/workspaces/docs/skills/deploy").await;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        assert_eq!(
            response.text().await.expect("read body"),
            "Workspace body.\n"
        );

        let document = server.get_json("/workspaces/docs/skills").await;
        assert_eq!(
            entry(&document, "deploy")["description"],
            "Workspace deploy."
        );
    }

    /// A workspace without its own `.agents` still answers with global's skills.
    #[tokio::test]
    async fn absent_workspace() {
        let home = TempDir::new("merge-absent-home");
        let root = TempDir::new("merge-absent-root");
        write_skill(
            &skills_dir(home.path()),
            "global-only",
            "name: global-only\ndescription: Discovered under the home directory.",
            "Global.\n",
        );

        let server = Server::start(home.path()).await;
        register(&server, "docs", root.path()).await;

        assert_eq!(
            ids(&server.get_json("/workspaces/docs/skills").await),
            ["global-only"]
        );
    }
}

mod list {
    use super::*;

    /// Every valid skill is listed once, in id order.
    #[tokio::test]
    async fn skills() {
        let home = TempDir::new("list-skills");
        let skills = skills_dir(home.path());
        write_skill(
            &skills,
            "gamma",
            "name: gamma\ndescription: Third.",
            "Third.\n",
        );
        write_skill(
            &skills,
            "deploy",
            "name: deploy\ndescription: First.",
            "First.\n",
        );

        let server = Server::start(home.path()).await;

        assert_eq!(ids(&server.get_json("/skills").await), ["deploy", "gamma"]);
    }

    /// An entry carries the frontmatter fields that were written, and no others.
    #[tokio::test]
    async fn fields() {
        let home = TempDir::new("list-fields");
        let skills = skills_dir(home.path());
        let frontmatter = concat!(
            "name: deploy\n",
            "description: Roll out a service.\n",
            "license: MIT\n",
            "compatibility: Requires kubectl\n",
            "metadata:\n",
            "  author: acme",
        );
        let deploy = write_skill(&skills, "deploy", frontmatter, "Ship it.\n");
        write_skill(
            &skills,
            "other",
            "name: other\ndescription: Another skill.",
            "Other.\n",
        );

        let server = Server::start(home.path()).await;
        let document = server.get_json("/skills").await;

        assert_eq!(
            entry(&document, "deploy"),
            &json!({
                "id": "deploy",
                "root": deploy,
                "name": "deploy",
                "description": "Roll out a service.",
                "license": "MIT",
                "compatibility": "Requires kubectl",
                "metadata": { "author": "acme" },
                "uri": server.url("/skills/deploy"),
                "workspace_id": "skill.global.deploy",
            })
        );

        let other = entry(&document, "other");
        assert!(other.get("license").is_none(), "{other}");
        assert!(other.get("compatibility").is_none(), "{other}");
        assert!(other.get("metadata").is_none(), "{other}");
    }

    /// `uri` addresses the body at the mount point that answered.
    #[tokio::test]
    async fn uri() {
        let home = TempDir::new("list-uri-home");
        let root = TempDir::new("list-uri-root");
        write_skill(
            &skills_dir(home.path()),
            "deploy",
            "name: deploy\ndescription: Global deploy.",
            "Global body.\n",
        );
        write_skill(
            &skills_dir(root.path()),
            "deploy",
            "name: deploy\ndescription: Workspace deploy.",
            "Workspace body.\n",
        );

        let server = Server::start(home.path()).await;
        register(&server, "docs", root.path()).await;

        let global = server.get_json("/skills").await;
        assert_eq!(
            entry(&global, "deploy")["uri"],
            server.url("/skills/deploy")
        );

        let scoped = server.get_json("/workspaces/docs/skills").await;
        assert_eq!(
            entry(&scoped, "deploy")["uri"],
            server.url("/workspaces/docs/skills/deploy")
        );
    }

    /// A workspace mount answers the merged document.
    #[tokio::test]
    async fn workspace() {
        let home = TempDir::new("list-workspace-home");
        let root = TempDir::new("list-workspace-root");
        write_skill(
            &skills_dir(home.path()),
            "global-only",
            "name: global-only\ndescription: Discovered under the home directory.",
            "Global.\n",
        );
        write_skill(
            &skills_dir(root.path()),
            "workspace-only",
            "name: workspace-only\ndescription: Registered with the workspace.",
            "Workspace.\n",
        );

        let server = Server::start(home.path()).await;
        register(&server, "docs", root.path()).await;

        let document = server.get_json("/workspaces/docs/skills").await;
        assert_eq!(ids(&document), ["global-only", "workspace-only"]);

        // A global skill keeps its own scope while being addressed through this mount.
        let global_only = entry(&document, "global-only");
        assert_eq!(global_only["workspace_id"], "skill.global.global-only");
        assert_eq!(
            global_only["uri"],
            server.url("/workspaces/docs/skills/global-only")
        );
    }

    /// An unknown workspace answers 404.
    #[tokio::test]
    async fn unknown_workspace() {
        let home = TempDir::new("list-unknown");
        write_skill(
            &skills_dir(home.path()),
            "deploy",
            "name: deploy\ndescription: Roll out a service.",
            "Ship it.\n",
        );

        let server = Server::start(home.path()).await;

        assert_eq!(
            server.get("/workspaces/missing/skills").await.status(),
            reqwest::StatusCode::NOT_FOUND
        );
    }
}

mod read {
    use super::*;

    /// `GET /skills/{id}` returns the text after the frontmatter as markdown.
    #[tokio::test]
    async fn body() {
        let home = TempDir::new("read-body");
        write_skill(
            &skills_dir(home.path()),
            "deploy",
            "name: deploy\ndescription: Roll out a service.",
            "Ship it.\n",
        );

        let server = Server::start(home.path()).await;
        let response = server.get("/skills/deploy").await;

        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let content_type = response.headers()[reqwest::header::CONTENT_TYPE]
            .to_str()
            .expect("content type")
            .to_owned();
        assert!(content_type.starts_with("text/markdown"), "{content_type}");
        assert_eq!(response.text().await.expect("read body"), "Ship it.\n");
    }

    /// A mount reads a skill that only global discovered.
    #[tokio::test]
    async fn merged() {
        let home = TempDir::new("read-merged-home");
        let root = TempDir::new("read-merged-root");
        write_skill(
            &skills_dir(home.path()),
            "global-only",
            "name: global-only\ndescription: Discovered under the home directory.",
            "Global body.\n",
        );

        let server = Server::start(home.path()).await;
        register(&server, "docs", root.path()).await;

        let response = server.get("/workspaces/docs/skills/global-only").await;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        assert_eq!(response.text().await.expect("read body"), "Global body.\n");
    }

    /// A skill that was not discovered answers 404.
    #[tokio::test]
    async fn not_found() {
        let home = TempDir::new("read-missing");
        write_skill(
            &skills_dir(home.path()),
            "deploy",
            "name: deploy\ndescription: Roll out a service.",
            "Ship it.\n",
        );

        let server = Server::start(home.path()).await;

        assert_eq!(
            server.get("/skills/missing").await.status(),
            reqwest::StatusCode::NOT_FOUND
        );
    }
}

mod workspace {
    use super::*;

    /// A skill derives a workspace rooted at its own directory.
    #[tokio::test]
    async fn resolves() {
        let home = TempDir::new("workspace-resolves-home");
        let root = TempDir::new("workspace-resolves-root");
        let deploy = write_skill(
            &skills_dir(root.path()),
            "deploy",
            "name: deploy\ndescription: Roll out a service.",
            "Ship it.\n",
        );

        let server = Server::start(home.path()).await;
        register(&server, "docs", root.path()).await;

        assert_eq!(
            server.get_json("/workspaces/skill.docs.deploy").await,
            json!({
                "id": "skill.docs.deploy",
                "root": deploy,
                "access": "read-write",
            })
        );
    }

    /// The derived id works at a workspace mount, reaching the skill's own files.
    #[tokio::test]
    async fn usable() {
        let home = TempDir::new("workspace-usable-home");
        let root = TempDir::new("workspace-usable-root");
        let deploy = write_skill(
            &skills_dir(root.path()),
            "deploy",
            "name: deploy\ndescription: Roll out a service.",
            "Ship it.\n",
        );
        std::fs::write(deploy.join("run.sh"), "#!/bin/sh\necho hi\n").expect("write run.sh");

        let server = Server::start(home.path()).await;
        register(&server, "docs", root.path()).await;

        let cwd = server
            .post(
                "/workspaces/skill.docs.deploy/exec",
                json!({ "command": "pwd", "wait": 5000 }),
            )
            .await
            .json::<Value>()
            .await
            .expect("decode exec result");
        assert_eq!(
            cwd["stdout"].as_str().expect("stdout").trim_end(),
            deploy.to_string_lossy()
        );

        let bundled = server
            .post(
                "/workspaces/skill.docs.deploy/exec",
                json!({ "command": "cat", "args": ["run.sh"], "wait": 5000 }),
            )
            .await
            .json::<Value>()
            .await
            .expect("decode exec result");
        assert_eq!(bundled["stdout"], "#!/bin/sh\necho hi\n");
        assert_eq!(bundled["exit_code"], 0);
    }

    /// The derived workspace reports the access of the scope it came from.
    #[tokio::test]
    async fn access() {
        let home = TempDir::new("workspace-access-home");
        let root = TempDir::new("workspace-access-root");
        write_skill(
            &skills_dir(root.path()),
            "deploy",
            "name: deploy\ndescription: Roll out a service.",
            "Ship it.\n",
        );

        let server = Server::start(home.path()).await;
        register_as(&server, "docs", root.path(), "read-only").await;

        assert_eq!(
            server.get_json("/workspaces/skill.docs.deploy").await["access"],
            "read-only"
        );
    }

    /// A derived workspace stays out of the registry listing.
    #[tokio::test]
    async fn not_listed() {
        let home = TempDir::new("workspace-not-listed-home");
        let root = TempDir::new("workspace-not-listed-root");
        write_skill(
            &skills_dir(root.path()),
            "deploy",
            "name: deploy\ndescription: Roll out a service.",
            "Ship it.\n",
        );

        let server = Server::start(home.path()).await;
        register(&server, "docs", root.path()).await;

        let listed = server.get_json("/workspaces").await;
        let ids: BTreeSet<String> = listed
            .as_array()
            .expect("the workspace list")
            .iter()
            .map(|workspace| workspace["id"].as_str().expect("workspace id").to_owned())
            .collect();

        assert_eq!(
            ids,
            BTreeSet::from(["docs".to_owned(), "global".to_owned()])
        );
    }

    /// An undiscovered skill, and a scope that is not registered, have no workspace.
    #[tokio::test]
    async fn unknown() {
        let home = TempDir::new("workspace-unknown-home");
        let root = TempDir::new("workspace-unknown-root");
        write_skill(
            &skills_dir(root.path()),
            "deploy",
            "name: deploy\ndescription: Roll out a service.",
            "Ship it.\n",
        );

        let server = Server::start(home.path()).await;
        register(&server, "docs", root.path()).await;

        assert_eq!(
            server.get("/workspaces/skill.docs.missing").await.status(),
            reqwest::StatusCode::NOT_FOUND
        );
        assert_eq!(
            server
                .get("/workspaces/skill.missing.deploy")
                .await
                .status(),
            reqwest::StatusCode::NOT_FOUND
        );
    }

    /// A derived workspace is neither updatable nor removable.
    #[tokio::test]
    async fn mutate_denied() {
        let home = TempDir::new("workspace-mutate-home");
        let root = TempDir::new("workspace-mutate-root");
        write_skill(
            &skills_dir(root.path()),
            "deploy",
            "name: deploy\ndescription: Roll out a service.",
            "Ship it.\n",
        );

        let server = Server::start(home.path()).await;
        register(&server, "docs", root.path()).await;

        assert_eq!(
            server
                .patch(
                    "/workspaces/skill.docs.deploy",
                    json!({ "access": "read-only" }),
                )
                .await
                .status(),
            reqwest::StatusCode::FORBIDDEN
        );
        assert_eq!(
            server
                .delete("/workspaces/skill.docs.deploy")
                .await
                .status(),
            reqwest::StatusCode::FORBIDDEN
        );
    }
}
