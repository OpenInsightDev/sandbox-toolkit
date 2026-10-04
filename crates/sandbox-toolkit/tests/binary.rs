mod harness;

use std::fs::File;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use harness::{Dir, Server};

const TOOLS: [&str; 4] = ["fd", "rg", "uv", "deno"];

/// A directory the server's own `PATH` keeps, so `path::keeps` can tell an added
/// entry from a `PATH` that was replaced.
const SENTINEL: &str = "/sbxtkt-test-sentinel-bin";

fn cache_bin(home: &Path) -> PathBuf {
    home.join(".cache/sandbox-toolkit/bin")
}

/// The blake3 digest of a materialized file, the signature the embedded
/// manifest carries.
fn digest(path: &Path) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher
        .update_reader(File::open(path).expect("open materialized file"))
        .expect("hash materialized file");
    hasher.finalize().to_hex().to_string()
}

fn digests(home: &Path) -> Vec<(String, String)> {
    TOOLS
        .iter()
        .map(|tool| ((*tool).to_owned(), digest(&cache_bin(home).join(tool))))
        .collect()
}

fn modified(home: &Path) -> Vec<(String, std::time::SystemTime)> {
    TOOLS
        .iter()
        .map(|tool| {
            let path = cache_bin(home).join(tool);
            let time = std::fs::metadata(&path)
                .expect("stat materialized file")
                .modified()
                .expect("materialized file has a mtime");
            ((*tool).to_owned(), time)
        })
        .collect()
}

fn assert_ran(result: &Value, what: &str) {
    assert_eq!(result["status"], "exited", "{what}");
    assert_eq!(result["exit_code"], 0, "{what}");
}

mod embed {
    use super::*;

    #[tokio::test]
    async fn tools() {
        let home = Dir::new("embed-tools");
        let server = Server::start(&home).await;

        for tool in TOOLS {
            // Absolute path, so what runs is the embedded payload itself.
            let path = cache_bin(home.path()).join(tool);
            let result = server
                .exec_json(json!({
                    "command": path.to_string_lossy(),
                    "args": ["--version"],
                    "wait": 30_000,
                }))
                .await;

            assert_ran(&result, tool);
            assert!(
                !result["stdout"].as_str().expect("stdout").trim().is_empty(),
                "{tool} printed no version"
            );
        }
    }
}

mod materialize {
    use super::*;

    #[tokio::test]
    async fn layout() {
        let home = Dir::new("materialize-layout");
        Server::start(&home).await;

        for tool in TOOLS {
            let path = cache_bin(home.path()).join(tool);
            let metadata = std::fs::metadata(&path)
                .unwrap_or_else(|error| panic!("{}: {error}", path.display()));

            assert!(metadata.is_file(), "{} is not a file", path.display());
            assert!(metadata.len() > 0, "{} is empty", path.display());
            assert!(
                metadata.permissions().mode() & 0o111 != 0,
                "{} is not executable",
                path.display()
            );
        }
    }

    #[tokio::test]
    async fn skip() {
        let home = Dir::new("materialize-skip");
        Server::start(&home).await;
        let before = digests(home.path());
        let times = modified(home.path());

        Server::start(&home).await;

        assert_eq!(digests(home.path()), before, "content changed on restart");
        assert_eq!(
            modified(home.path()),
            times,
            "files were rewritten on restart"
        );
    }

    #[tokio::test]
    async fn repair() {
        let home = Dir::new("materialize-repair");
        Server::start(&home).await;
        let before = digests(home.path());

        let tampered = cache_bin(home.path()).join("fd");
        let removed = cache_bin(home.path()).join("rg");
        std::fs::write(&tampered, b"not the embedded tool").expect("tamper with a tool");
        std::fs::remove_file(&removed).expect("delete a tool");

        Server::start(&home).await;

        assert_eq!(digests(home.path()), before, "tools were not restored");
    }
}

mod path {
    use super::*;

    #[tokio::test]
    async fn tools() {
        let home = Dir::new("path-tools");
        let server = Server::start(&home).await;

        for tool in TOOLS {
            let result = server
                .exec_json(json!({ "command": tool, "args": ["--version"], "wait": 30_000 }))
                .await;

            assert_ran(&result, tool);
            assert!(
                !result["stdout"].as_str().expect("stdout").trim().is_empty(),
                "{tool} printed no version"
            );
        }
    }

    #[tokio::test]
    async fn keeps() {
        let home = Dir::new("path-keeps");
        let server = Server::start_with(&home, |command| {
            command.env("PATH", format!("{SENTINEL}:/usr/bin:/bin"));
        })
        .await;

        let result = server
            .exec_json(json!({
                "format": "shell",
                "script": "printf %s \"$PATH\"",
                "wait": 30_000,
            }))
            .await;
        assert_ran(&result, "PATH");

        let path = result["stdout"].as_str().expect("stdout").to_owned();
        let entries: Vec<&str> = path.split(':').collect();
        assert!(
            entries.contains(&SENTINEL),
            "the server's own PATH entry was replaced: {path}"
        );
        assert!(
            entries
                .iter()
                .any(|entry| Path::new(entry) == cache_bin(home.path())),
            "the materialized directory is not on PATH: {path}"
        );
    }
}
