use std::fs::File;
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// The tools the build embeds, named as the design names them.
const TOOLS: [&str; 4] = ["fd", "rg", "uv", "deno"];

/// A directory the server's own `PATH` keeps, so `path::keeps` can tell an added
/// entry from a `PATH` that was replaced.
const SENTINEL: &str = "/sbxtkt-test-sentinel-bin";

/// A directory removed when the test ends.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let serial = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "sbxtkt-binary-{tag}-{}-{serial}",
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

/// The `sbxtkt` server under test, running on a private port with a throwaway
/// home.
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
            // The cache the tools materialize into follows `HOME`, not whatever
            // cache the developer running the tests has set.
            .env_remove("XDG_CACHE_HOME")
            .env("PATH", format!("{SENTINEL}:/usr/bin:/bin"))
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

    async fn post(&self, path: &str, body: Value) -> reqwest::Response {
        reqwest::Client::new()
            .post(self.url(path))
            .json(&body)
            .send()
            .await
            .expect("POST request")
    }

    /// The decoded direct result of a command, asserting it succeeded.
    async fn exec_json(&self, body: Value) -> Value {
        let response = self.post("/exec", body).await;
        assert_eq!(response.status(), reqwest::StatusCode::OK, "POST /exec");
        response.json().await.expect("decode exec result")
    }

    async fn wait_ready(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(60);
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

/// Where the tools materialize: the system cache dir of `home`.
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

/// Asserts a command that ran to completion succeeded.
fn assert_ran(result: &Value, what: &str) {
    assert_eq!(result["status"], "exited", "{what}");
    assert_eq!(result["exit_code"], 0, "{what}");
}

mod embed {
    use super::*;

    #[tokio::test]
    async fn tools() {
        let home = TempDir::new("embed-tools");
        let server = Server::start(home.path()).await;

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
        let home = TempDir::new("materialize-layout");
        Server::start(home.path()).await;

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
        let home = TempDir::new("materialize-skip");
        Server::start(home.path()).await;
        let before = digests(home.path());
        let times = modified(home.path());

        Server::start(home.path()).await;

        assert_eq!(digests(home.path()), before, "content changed on restart");
        assert_eq!(
            modified(home.path()),
            times,
            "files were rewritten on restart"
        );
    }

    #[tokio::test]
    async fn repair() {
        let home = TempDir::new("materialize-repair");
        Server::start(home.path()).await;
        let before = digests(home.path());

        let tampered = cache_bin(home.path()).join("fd");
        let removed = cache_bin(home.path()).join("rg");
        std::fs::write(&tampered, b"not the embedded tool").expect("tamper with a tool");
        std::fs::remove_file(&removed).expect("delete a tool");

        Server::start(home.path()).await;

        assert_eq!(digests(home.path()), before, "tools were not restored");
    }
}

mod path {
    use super::*;

    #[tokio::test]
    async fn tools() {
        let home = TempDir::new("path-tools");
        let server = Server::start(home.path()).await;

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
        let home = TempDir::new("path-keeps");
        let server = Server::start(home.path()).await;

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
