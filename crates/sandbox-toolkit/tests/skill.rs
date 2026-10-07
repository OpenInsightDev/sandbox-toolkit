mod harness;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use harness::{Dir, Server, manifest, write_plugin, write_plugin_skill};

fn skills_dir(root: &Path) -> PathBuf {
    root.join(".agents/skills")
}

fn write_skill(skills: &Path, id: &str, frontmatter: &str, body: &str) -> PathBuf {
    let dir = skills.join(id);
    std::fs::create_dir_all(&dir).expect("create the skill directory");
    let document = format!("---\n{frontmatter}\n---\n\n{body}");
    std::fs::write(dir.join("SKILL.md"), document).expect("write SKILL.md");
    dir
}

async fn register_as(server: &Server, id: &str, root: &Path, access: &str) {
    let body = json!({ "id": id, "root": root.to_string_lossy(), "access": access });
    let response = server.post("/workspaces", body).await;
    assert_eq!(
        response.status(),
        reqwest::StatusCode::CREATED,
        "register workspace {id}"
    );
}

fn ids(document: &Value) -> Vec<String> {
    document["skills"]
        .as_array()
        .expect("the skills array")
        .iter()
        .map(|entry| entry["id"].as_str().expect("skill id").to_owned())
        .collect()
}

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

    #[tokio::test]
    async fn discovers() {
        let home = Dir::new("load-discovers");
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

        let server = Server::start(&home).await;

        assert_eq!(ids(&server.get_json("/skills").await), ["deploy", "other"]);
    }

    #[tokio::test]
    async fn plugin() {
        let home = Dir::new("load-plugin");
        let plugin = write_plugin(home.path(), "deploy-kit", manifest("deploy-kit"));
        write_plugin_skill(
            &plugin,
            "deploy",
            "name: deploy\ndescription: Shipped by a plugin.",
            "Plugin body.\n",
        );

        let server = Server::start(&home).await;

        assert_eq!(ids(&server.get_json("/skills").await), ["deploy-kit.deploy"]);
    }

    #[tokio::test]
    async fn skips() {
        let home = Dir::new("load-skips");
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

        let server = Server::start(&home).await;

        assert_eq!(ids(&server.get_json("/skills").await), ["deploy"]);
    }

    #[tokio::test]
    async fn rescans() {
        let home = Dir::new("load-rescans");
        let skills = skills_dir(home.path());
        write_skill(
            &skills,
            "deploy",
            "name: deploy\ndescription: Roll out a service.",
            "Ship it.\n",
        );

        let server = Server::start(&home).await;

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

    #[tokio::test]
    async fn includes_global() {
        let home = Dir::new("merge-includes-home");
        let root = Dir::new("merge-includes-root");
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

        let server = Server::start(&home).await;
        server.register("docs", root.path()).await;

        assert_eq!(
            ids(&server.get_json("/workspaces/docs/skills").await),
            ["global-only", "workspace-only"]
        );
    }

    #[tokio::test]
    async fn workspace_wins() {
        let home = Dir::new("merge-wins-home");
        let root = Dir::new("merge-wins-root");
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

        let server = Server::start(&home).await;
        server.register("docs", root.path()).await;

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

    /// A plugin's skill carries the plugin id as a prefix, so the only way two
    /// entries collide is a same-named plugin in both scopes.
    #[tokio::test]
    async fn plugin_prefix() {
        let home = Dir::new("merge-plugin-home");
        let root = Dir::new("merge-plugin-root");
        let global = write_plugin(home.path(), "deploy-kit", manifest("deploy-kit"));
        write_plugin_skill(
            &global,
            "deploy",
            "name: deploy\ndescription: Shipped by the global plugin.",
            "Global plugin body.\n",
        );
        let local = write_plugin(root.path(), "deploy-kit", manifest("deploy-kit"));
        write_plugin_skill(
            &local,
            "deploy",
            "name: deploy\ndescription: Shipped by the workspace plugin.",
            "Workspace plugin body.\n",
        );

        let server = Server::start(&home).await;
        server.register("docs", root.path()).await;

        assert_eq!(
            ids(&server.get_json("/workspaces/docs/skills").await),
            ["deploy-kit.deploy"]
        );

        let response = server.get("/workspaces/docs/skills/deploy-kit.deploy").await;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        assert_eq!(
            response.text().await.expect("read body"),
            "Workspace plugin body.\n"
        );
    }

    #[tokio::test]
    async fn absent_workspace() {
        let home = Dir::new("merge-absent-home");
        let root = Dir::new("merge-absent-root");
        write_skill(
            &skills_dir(home.path()),
            "global-only",
            "name: global-only\ndescription: Discovered under the home directory.",
            "Global.\n",
        );

        let server = Server::start(&home).await;
        server.register("docs", root.path()).await;

        assert_eq!(
            ids(&server.get_json("/workspaces/docs/skills").await),
            ["global-only"]
        );
    }
}

mod list {
    use super::*;

    #[tokio::test]
    async fn skills() {
        let home = Dir::new("list-skills");
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

        let server = Server::start(&home).await;

        assert_eq!(ids(&server.get_json("/skills").await), ["deploy", "gamma"]);
    }

    #[tokio::test]
    async fn fields() {
        let home = Dir::new("list-fields");
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

        let server = Server::start(&home).await;
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

    #[tokio::test]
    async fn uri() {
        let home = Dir::new("list-uri-home");
        let root = Dir::new("list-uri-root");
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

        let server = Server::start(&home).await;
        server.register("docs", root.path()).await;

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

    #[tokio::test]
    async fn workspace() {
        let home = Dir::new("list-workspace-home");
        let root = Dir::new("list-workspace-root");
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

        let server = Server::start(&home).await;
        server.register("docs", root.path()).await;

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

    #[tokio::test]
    async fn unknown_workspace() {
        let home = Dir::new("list-unknown");
        write_skill(
            &skills_dir(home.path()),
            "deploy",
            "name: deploy\ndescription: Roll out a service.",
            "Ship it.\n",
        );

        let server = Server::start(&home).await;

        assert_eq!(
            server.get("/workspaces/missing/skills").await.status(),
            reqwest::StatusCode::NOT_FOUND
        );
    }
}

mod read {
    use super::*;

    #[tokio::test]
    async fn body() {
        let home = Dir::new("read-body");
        write_skill(
            &skills_dir(home.path()),
            "deploy",
            "name: deploy\ndescription: Roll out a service.",
            "Ship it.\n",
        );

        let server = Server::start(&home).await;
        let response = server.get("/skills/deploy").await;

        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let content_type = response.headers()[reqwest::header::CONTENT_TYPE]
            .to_str()
            .expect("content type")
            .to_owned();
        assert!(content_type.starts_with("text/markdown"), "{content_type}");
        assert_eq!(response.text().await.expect("read body"), "Ship it.\n");
    }

    #[tokio::test]
    async fn merged() {
        let home = Dir::new("read-merged-home");
        let root = Dir::new("read-merged-root");
        write_skill(
            &skills_dir(home.path()),
            "global-only",
            "name: global-only\ndescription: Discovered under the home directory.",
            "Global body.\n",
        );

        let server = Server::start(&home).await;
        server.register("docs", root.path()).await;

        let response = server.get("/workspaces/docs/skills/global-only").await;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        assert_eq!(response.text().await.expect("read body"), "Global body.\n");
    }

    #[tokio::test]
    async fn not_found() {
        let home = Dir::new("read-missing");
        write_skill(
            &skills_dir(home.path()),
            "deploy",
            "name: deploy\ndescription: Roll out a service.",
            "Ship it.\n",
        );

        let server = Server::start(&home).await;

        assert_eq!(
            server.get("/skills/missing").await.status(),
            reqwest::StatusCode::NOT_FOUND
        );
    }
}

mod workspace {
    use super::*;

    #[tokio::test]
    async fn resolves() {
        let home = Dir::new("workspace-resolves-home");
        let root = Dir::new("workspace-resolves-root");
        let deploy = write_skill(
            &skills_dir(root.path()),
            "deploy",
            "name: deploy\ndescription: Roll out a service.",
            "Ship it.\n",
        );

        let server = Server::start(&home).await;
        server.register("docs", root.path()).await;

        assert_eq!(
            server.get_json("/workspaces/skill.docs.deploy").await,
            json!({
                "id": "skill.docs.deploy",
                "root": deploy,
                "access": "read-write",
            })
        );
    }

    #[tokio::test]
    async fn usable() {
        let home = Dir::new("workspace-usable-home");
        let root = Dir::new("workspace-usable-root");
        let deploy = write_skill(
            &skills_dir(root.path()),
            "deploy",
            "name: deploy\ndescription: Roll out a service.",
            "Ship it.\n",
        );
        std::fs::write(deploy.join("run.sh"), "#!/bin/sh\necho hi\n").expect("write run.sh");

        let server = Server::start(&home).await;
        server.register("docs", root.path()).await;

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

    #[tokio::test]
    async fn access() {
        let home = Dir::new("workspace-access-home");
        let root = Dir::new("workspace-access-root");
        write_skill(
            &skills_dir(root.path()),
            "deploy",
            "name: deploy\ndescription: Roll out a service.",
            "Ship it.\n",
        );

        let server = Server::start(&home).await;
        register_as(&server, "docs", root.path(), "read-only").await;

        assert_eq!(
            server.get_json("/workspaces/skill.docs.deploy").await["access"],
            "read-only"
        );
    }

    #[tokio::test]
    async fn not_listed() {
        let home = Dir::new("workspace-not-listed-home");
        let root = Dir::new("workspace-not-listed-root");
        write_skill(
            &skills_dir(root.path()),
            "deploy",
            "name: deploy\ndescription: Roll out a service.",
            "Ship it.\n",
        );

        let server = Server::start(&home).await;
        server.register("docs", root.path()).await;

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

    #[tokio::test]
    async fn unknown() {
        let home = Dir::new("workspace-unknown-home");
        let root = Dir::new("workspace-unknown-root");
        write_skill(
            &skills_dir(root.path()),
            "deploy",
            "name: deploy\ndescription: Roll out a service.",
            "Ship it.\n",
        );

        let server = Server::start(&home).await;
        server.register("docs", root.path()).await;

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

    #[tokio::test]
    async fn plugin_skill() {
        let home = Dir::new("workspace-plugin-home");
        let plugin = write_plugin(home.path(), "deploy-kit", manifest("deploy-kit"));
        let deploy = write_plugin_skill(
            &plugin,
            "deploy",
            "name: deploy\ndescription: Shipped by a plugin.",
            "Plugin body.\n",
        );

        let server = Server::start(&home).await;

        assert_eq!(
            server
                .get_json("/workspaces/skill.global.deploy-kit.deploy")
                .await,
            json!({
                "id": "skill.global.deploy-kit.deploy",
                "root": deploy,
                "access": "read-write",
            })
        );
    }

    #[tokio::test]
    async fn mutate_denied() {
        let home = Dir::new("workspace-mutate-home");
        let root = Dir::new("workspace-mutate-root");
        write_skill(
            &skills_dir(root.path()),
            "deploy",
            "name: deploy\ndescription: Roll out a service.",
            "Ship it.\n",
        );

        let server = Server::start(&home).await;
        server.register("docs", root.path()).await;

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
