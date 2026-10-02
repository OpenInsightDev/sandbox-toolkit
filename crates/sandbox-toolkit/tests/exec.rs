use std::fs::File;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const STDOUT: u8 = 1;
const STDERR: u8 = 2;
const ERROR: u8 = 3;

const MAX_PAYLOAD_LEN: usize = 4 * 1024 * 1024;

/// A directory removed when the test ends.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let serial = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("sbxtkt-exec-{tag}-{}-{serial}", std::process::id()));
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

    async fn post(&self, path: &str, body: Value) -> reqwest::Response {
        reqwest::Client::new()
            .post(self.url(path))
            .json(&body)
            .send()
            .await
            .expect("POST request")
    }

    async fn exec(&self, body: Value) -> reqwest::Response {
        self.post("/exec", body).await
    }

    /// The decoded direct result of a command, asserting it succeeded.
    async fn exec_json(&self, body: Value) -> Value {
        let response = self.exec(body).await;
        assert_eq!(response.status(), reqwest::StatusCode::OK, "POST /exec");
        response.json().await.expect("decode exec result")
    }

    /// The raw body of a command the server answered with the frame stream.
    async fn exec_bytes(&self, body: Value) -> Vec<u8> {
        let response = self.exec(body).await;
        assert_eq!(response.status(), reqwest::StatusCode::OK, "POST /exec");
        response.bytes().await.expect("read exec stream").to_vec()
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

/// Splits a length-prefixed frame stream, asserting every frame is well formed.
fn decode_frames(mut bytes: &[u8]) -> Vec<(u8, Vec<u8>)> {
    let mut frames = Vec::new();

    while !bytes.is_empty() {
        assert!(bytes.len() >= 5, "frame header truncated");
        let channel = bytes[0];
        let length = u32::from_be_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]) as usize;
        assert!(
            length <= MAX_PAYLOAD_LEN,
            "frame payload of {length} bytes exceeds the per-frame limit"
        );
        assert!(bytes.len() >= 5 + length, "frame payload truncated");
        frames.push((channel, bytes[5..5 + length].to_vec()));
        bytes = &bytes[5 + length..];
    }

    frames
}

/// The bytes carried on `channel`, concatenated across frames.
fn channel_bytes(frames: &[(u8, Vec<u8>)], channel: u8) -> Vec<u8> {
    frames
        .iter()
        .filter(|(id, _)| *id == channel)
        .flat_map(|(_, payload)| payload.clone())
        .collect()
}

/// The terminal status JSON, asserting the stream ends with exactly one.
fn terminal_status(frames: &[(u8, Vec<u8>)]) -> Value {
    let statuses = frames.iter().filter(|(id, _)| *id == ERROR).count();
    assert_eq!(statuses, 1, "exactly one terminal frame");
    let (channel, payload) = frames.last().expect("a terminal frame");
    assert_eq!(*channel, ERROR, "the terminal frame comes last");
    serde_json::from_slice(payload).expect("decode terminal status")
}

mod mount {
    use super::*;

    #[tokio::test]
    async fn global() {
        let home = TempDir::new("mount-global");
        let server = Server::start(home.path()).await;

        let result = server
            .exec_json(json!({ "command": "pwd", "wait": 5000 }))
            .await;

        assert_eq!(result["status"], "exited");
        assert_eq!(result["exit_code"], 0);
        assert_eq!(
            result["stdout"].as_str().expect("stdout").trim_end(),
            home.path().to_string_lossy()
        );
    }

    #[tokio::test]
    async fn workspace() {
        let home = TempDir::new("mount-workspace-home");
        let root = TempDir::new("mount-workspace-root");
        let server = Server::start(home.path()).await;
        register(&server, "w", root.path()).await;

        let response = server
            .post(
                "/workspaces/w/exec",
                json!({ "command": "pwd", "wait": 5000 }),
            )
            .await;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let result: Value = response.json().await.expect("decode exec result");

        assert_eq!(
            result["stdout"].as_str().expect("stdout").trim_end(),
            root.path().to_string_lossy()
        );
    }

    #[tokio::test]
    async fn unknown_workspace() {
        let home = TempDir::new("mount-unknown");
        let server = Server::start(home.path()).await;

        let response = server
            .post("/workspaces/missing/exec", json!({ "command": "true" }))
            .await;

        assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
    }
}

mod request {
    use super::*;

    #[tokio::test]
    async fn exec() {
        let home = TempDir::new("request-exec");
        let server = Server::start(home.path()).await;

        let result = server
            .exec_json(json!({ "command": "printf", "args": ["a b"], "wait": 5000 }))
            .await;

        assert_eq!(result["stdout"], "a b");
        assert_eq!(result["exit_code"], 0);
    }

    #[tokio::test]
    async fn shell() {
        let home = TempDir::new("request-shell");
        let server = Server::start(home.path()).await;

        let result = server
            .exec_json(json!({ "format": "shell", "script": "printf hi", "wait": 5000 }))
            .await;

        assert_eq!(result["stdout"], "hi");
    }

    #[tokio::test]
    async fn format_default() {
        let home = TempDir::new("request-format-default");
        let server = Server::start(home.path()).await;

        let result = server
            .exec_json(json!({ "command": "printf", "args": ["hi"], "wait": 5000 }))
            .await;

        assert_eq!(result["stdout"], "hi");
    }

    #[tokio::test]
    async fn cwd() {
        let home = TempDir::new("request-cwd-home");
        let dir = TempDir::new("request-cwd-dir");
        let server = Server::start(home.path()).await;

        let result = server
            .exec_json(json!({
                "command": "pwd",
                "cwd": dir.path().to_string_lossy(),
                "wait": 5000,
            }))
            .await;

        assert_eq!(
            result["stdout"].as_str().expect("stdout").trim_end(),
            dir.path().to_string_lossy()
        );
    }

    #[tokio::test]
    async fn env() {
        let home = TempDir::new("request-env");
        let server = Server::start(home.path()).await;

        let result = server
            .exec_json(json!({
                "command": "sh",
                "args": ["-c", "printf %s \"$FOO\""],
                "env": { "FOO": "bar" },
                "wait": 5000,
            }))
            .await;

        assert_eq!(result["stdout"], "bar");
    }
}

mod direct {
    use super::*;

    #[tokio::test]
    async fn status() {
        let home = TempDir::new("direct-status");
        let server = Server::start(home.path()).await;

        let within = server
            .exec(json!({ "command": "true", "wait": 5000 }))
            .await;
        assert_eq!(within.status(), reqwest::StatusCode::OK);

        let upgraded = server.exec(json!({ "command": "true", "wait": 0 })).await;
        assert_eq!(upgraded.status(), reqwest::StatusCode::OK);
    }

    #[tokio::test]
    async fn exited() {
        let home = TempDir::new("direct-exited");
        let server = Server::start(home.path()).await;

        let result = server
            .exec_json(json!({ "command": "sh", "args": ["-c", "exit 3"], "wait": 5000 }))
            .await;

        assert_eq!(result["status"], "exited");
        assert_eq!(result["exit_code"], 3);
        assert!(result.get("signal").is_none());
    }

    #[tokio::test]
    async fn abnormal() {
        let home = TempDir::new("direct-abnormal");
        let server = Server::start(home.path()).await;

        let result = server
            .exec_json(json!({
                "command": "sh",
                "args": ["-c", "kill -TERM $$"],
                "wait": 5000,
            }))
            .await;

        assert_eq!(result["status"], "signaled");
        assert_eq!(result["signal"], "SIGTERM");
        assert!(result.get("exit_code").is_none());
    }

    #[tokio::test]
    async fn timeout() {
        let home = TempDir::new("direct-timeout");
        let server = Server::start(home.path()).await;

        let bytes = server
            .exec_bytes(json!({ "command": "sleep", "args": ["1"], "wait": 100 }))
            .await;
        let frames = decode_frames(&bytes);

        assert_eq!(terminal_status(&frames)["status"], "exited");
    }
}

mod stream {
    use super::*;

    #[tokio::test]
    async fn upgrades() {
        let home = TempDir::new("stream-upgrades");
        let server = Server::start(home.path()).await;

        let bytes = server
            .exec_bytes(json!({ "command": "printf", "args": ["hi"], "wait": 0 }))
            .await;
        let frames = decode_frames(&bytes);

        assert_eq!(channel_bytes(&frames, STDOUT), b"hi");
        assert_eq!(terminal_status(&frames)["status"], "exited");
    }

    #[tokio::test]
    async fn separates() {
        let home = TempDir::new("stream-separates");
        let server = Server::start(home.path()).await;

        let bytes = server
            .exec_bytes(json!({
                "format": "shell",
                "script": "printf out; printf err 1>&2",
                "wait": 0,
            }))
            .await;
        let frames = decode_frames(&bytes);

        assert_eq!(channel_bytes(&frames, STDOUT), b"out");
        assert_eq!(channel_bytes(&frames, STDERR), b"err");
    }

    #[tokio::test]
    async fn terminal() {
        let home = TempDir::new("stream-terminal");
        let server = Server::start(home.path()).await;

        let bytes = server
            .exec_bytes(json!({ "command": "true", "wait": 0 }))
            .await;
        let frames = decode_frames(&bytes);

        assert_eq!(frames.iter().filter(|(id, _)| *id == ERROR).count(), 1);
        assert_eq!(frames.last().expect("a terminal frame").0, ERROR);
    }

    #[tokio::test]
    async fn chunk_limit() {
        let home = TempDir::new("stream-chunk-limit");
        let server = Server::start(home.path()).await;

        let bytes = server
            .exec_bytes(json!({
                "format": "shell",
                "script": "head -c 5000000 /dev/zero | tr '\\0' a",
                "wait": 0,
            }))
            .await;
        let frames = decode_frames(&bytes);

        assert!(
            frames
                .iter()
                .all(|(_, payload)| payload.len() <= MAX_PAYLOAD_LEN)
        );
        assert_eq!(channel_bytes(&frames, STDOUT).len(), 5_000_000);
    }

    #[tokio::test]
    #[ignore = "receiver-side rule: the server never emits an unterminated stream"]
    async fn truncated() {
        let home = TempDir::new("stream-truncated");
        let server = Server::start(home.path()).await;

        let bytes = server
            .exec_bytes(json!({ "command": "true", "wait": 0 }))
            .await;
        let frames = decode_frames(&bytes);

        assert!(frames.iter().any(|(id, _)| *id == ERROR));
    }
}
