#![allow(dead_code)]

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU16, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use reqwest::Method;
use serde_json::{Value, json};

// The channel numbering exec's frame stream and pty's messages share: 0 stdin,
// 1 stdout, 2 stderr, 3 error, 4 resize.
pub const STDIN: u8 = 0;
pub const STDOUT: u8 = 1;
pub const STDERR: u8 = 2;
pub const ERROR: u8 = 3;
pub const RESIZE: u8 = 4;

/// The most bytes one payload carries; a longer payload is a protocol error.
pub const MAX_PAYLOAD_LEN: usize = 4 * 1024 * 1024;

pub const SCHEMA: &str = "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json";

/// The canonical `$schema` of an Agent Plugins 1.0.0 `plugin.json`.
pub const PLUGIN_SCHEMA: &str = "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json";

/// The first port a test process hands out. The block sits below the range the
/// kernel gives to connections, so a server never meets one of them.
const FIRST_PORT: u16 = 20_000;

/// How long the harness waits for a server to start, or to stop when asked.
const TIMEOUT: Duration = Duration::from_secs(30);

pub struct Dir(PathBuf);

impl Dir {
    pub fn new(tag: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);

        let serial = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("sbxtkt-tests-{}", std::process::id()));
        let path = root.join(format!("{tag}-{serial}"));
        std::fs::create_dir_all(&path).expect("create a test directory");

        Self(std::fs::canonicalize(path).expect("canonicalize a test directory"))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub struct Server {
    child: Child,
    base_url: String,
    log: PathBuf,
}

impl Server {
    pub async fn start(home: &Dir) -> Self {
        Self::start_with(home, |_| {}).await
    }

    pub async fn start_with(home: &Dir, configure: impl Fn(&mut Command)) -> Self {
        require_linux();

        let port = free_port();
        let log = home.path().join("server.log");
        let mut command = Command::new(env!("CARGO_BIN_EXE_sbxtkt"));
        command
            .args(["serve", "--host", "127.0.0.1", "--port", &port.to_string()])
            .env("HOME", home.path())
            // The cache the tools materialize into follows `HOME`, not the
            // cache of whoever runs the tests.
            .env_remove("XDG_CACHE_HOME")
            .env("RUST_LOG", "warn")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(
                File::create(&log).expect("create the server log"),
            ));
        configure(&mut command);

        let mut server = Self {
            child: command.spawn().expect("spawn sbxtkt"),
            base_url: format!("http://127.0.0.1:{port}"),
            log,
        };
        server.wait_ready().await;
        server
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    pub fn authority(&self) -> String {
        self.base_url.trim_start_matches("http://").to_owned()
    }

    pub async fn get(&self, path: &str) -> reqwest::Response {
        reqwest::get(self.url(path)).await.expect("GET request")
    }

    pub async fn post(&self, path: &str, body: Value) -> reqwest::Response {
        self.send_json(Method::POST, path, body).await
    }

    pub async fn patch(&self, path: &str, body: Value) -> reqwest::Response {
        self.send_json(Method::PATCH, path, body).await
    }

    pub async fn delete(&self, path: &str) -> reqwest::Response {
        reqwest::Client::new()
            .delete(self.url(path))
            .send()
            .await
            .expect("DELETE request")
    }

    pub async fn send_json(&self, method: Method, path: &str, body: Value) -> reqwest::Response {
        reqwest::Client::new()
            .request(method, self.url(path))
            .json(&body)
            .send()
            .await
            .expect("send request")
    }

    pub async fn get_json(&self, path: &str) -> Value {
        let response = self.get(path).await;
        assert_eq!(response.status(), reqwest::StatusCode::OK, "GET {path}");
        response.json().await.expect("decode GET body")
    }

    pub async fn exec(&self, body: Value) -> reqwest::Response {
        self.post("/exec", body).await
    }

    pub async fn exec_json(&self, body: Value) -> Value {
        let response = self.exec(body).await;
        assert_eq!(response.status(), reqwest::StatusCode::OK, "POST /exec");
        response.json().await.expect("decode exec result")
    }

    pub async fn exec_bytes(&self, body: Value) -> Vec<u8> {
        let response = self.exec(body).await;
        assert_eq!(response.status(), reqwest::StatusCode::OK, "POST /exec");
        response.bytes().await.expect("read exec stream").to_vec()
    }

    pub async fn register(&self, id: &str, root: &Path) {
        let body = json!({ "id": id, "root": root.to_string_lossy() });
        let response = self.post("/workspaces", body).await;
        assert_eq!(
            response.status(),
            reqwest::StatusCode::CREATED,
            "register workspace {id}"
        );
    }

    pub async fn terminate(&mut self) {
        self.signal();

        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            if self.child.try_wait().expect("poll the server").is_some() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("the server did not stop on SIGTERM");
    }

    /// Waits until `path` answers `status`, which a reload only reaches after
    /// the server has seen the change.
    pub async fn wait_status(&self, path: &str, status: reqwest::StatusCode) {
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            if self.get(path).await.status() == status {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("{path} did not answer {status}");
    }

    async fn wait_ready(&mut self) {
        let deadline = Instant::now() + TIMEOUT;

        while Instant::now() < deadline {
            if reqwest::get(self.url("/workspaces")).await.is_ok() {
                return;
            }
            if let Some(status) = self.child.try_wait().expect("poll the server") {
                panic!("the server exited with {status}: {}", self.log());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        panic!("the server did not become ready: {}", self.log());
    }

    fn log(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    fn signal(&self) {
        // SAFETY: the pid names this child.
        unsafe { libc::kill(self.child.id() as libc::pid_t, libc::SIGTERM) };
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // A kill would leave the sidecars behind, so the server is asked first.
        self.signal();

        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            if self.child.try_wait().is_ok_and(|status| status.is_some()) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn decode_frames(mut bytes: &[u8]) -> Vec<(u8, Vec<u8>)> {
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

pub fn channel_bytes(frames: &[(u8, Vec<u8>)], channel: u8) -> Vec<u8> {
    frames
        .iter()
        .filter(|(id, _)| *id == channel)
        .flat_map(|(_, payload)| payload.clone())
        .collect()
}

pub fn terminal_status(frames: &[(u8, Vec<u8>)]) -> Value {
    let statuses = frames.iter().filter(|(id, _)| *id == ERROR).count();
    assert_eq!(statuses, 1, "exactly one terminal frame");
    let (channel, payload) = frames.last().expect("a terminal frame");
    assert_eq!(*channel, ERROR, "the terminal frame comes last");
    serde_json::from_slice(payload).expect("decode terminal status")
}

pub fn write_mcp_json(root: &Path, servers: Value) {
    let agents = root.join(".agents");
    std::fs::create_dir_all(&agents).expect("create .agents");
    let document = json!({ "$schema": SCHEMA, "mcpServers": servers });
    std::fs::write(agents.join("mcp.json"), document.to_string()).expect("write mcp.json");
}

/// A minimal valid manifest for the plugin `name`.
pub fn manifest(name: &str) -> Value {
    json!({ "$schema": PLUGIN_SCHEMA, "name": name })
}

/// The directory a scope discovers its plugins in.
pub fn plugins_dir(root: &Path) -> PathBuf {
    root.join(".agents/plugins")
}

/// Writes `manifest` as a plugin's `plugin.json`, and returns the plugin
/// directory.
pub fn write_plugin(root: &Path, dir: &str, manifest: Value) -> PathBuf {
    let plugin = plugins_dir(root).join(dir);
    std::fs::create_dir_all(&plugin).expect("create the plugin directory");
    std::fs::write(plugin.join("plugin.json"), manifest.to_string()).expect("write plugin.json");
    plugin
}

/// Writes a plugin's `mcp.json`.
pub fn write_plugin_mcp(plugin: &Path, servers: Value) {
    let document = json!({ "$schema": SCHEMA, "mcpServers": servers });
    std::fs::write(plugin.join("mcp.json"), document.to_string()).expect("write mcp.json");
}

/// Writes a skill under a plugin's `skills/`, and returns its directory.
pub fn write_plugin_skill(plugin: &Path, id: &str, frontmatter: &str, body: &str) -> PathBuf {
    let dir = plugin.join("skills").join(id);
    std::fs::create_dir_all(&dir).expect("create the skill directory");
    let document = format!("---\n{frontmatter}\n---\n\n{body}");
    std::fs::write(dir.join("SKILL.md"), document).expect("write SKILL.md");
    dir
}

/// The canonical form of `path`, as the server reports a `root`.
pub fn canonical(path: &Path) -> String {
    std::fs::canonicalize(path)
        .expect("canonicalize a path")
        .to_string_lossy()
        .into_owned()
}

#[cfg(not(target_os = "linux"))]
fn require_linux() {
    panic!("the end-to-end tests run in the Linux container `vp run test` starts");
}

#[cfg(target_os = "linux")]
fn require_linux() {}

/// A free port for a server to bind.
///
/// One port per server, drawn from a block below the range the kernel gives to
/// connections, so two servers started at the same time never pick the same
/// one.
fn free_port() -> u16 {
    static NEXT: AtomicU16 = AtomicU16::new(FIRST_PORT);

    NEXT.fetch_add(1, Ordering::Relaxed)
}
