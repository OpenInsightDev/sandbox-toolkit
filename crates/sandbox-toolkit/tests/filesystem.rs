use std::fs::File;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use reqwest::Method;
use serde_json::{Value, json};

/// A directory removed when the test ends.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let serial = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("sbxtkt-fs-{tag}-{}-{serial}", std::process::id()));
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

    /// A request whose parameters travel in the JSON body.
    async fn send_json(&self, method: Method, path: &str, body: Value) -> reqwest::Response {
        reqwest::Client::new()
            .request(method, self.url(path))
            .json(&body)
            .send()
            .await
            .expect("send request")
    }

    /// A request whose body is the resource itself.
    async fn send_raw(&self, method: Method, path: &str, bytes: &[u8]) -> reqwest::Response {
        reqwest::Client::new()
            .request(method, self.url(path))
            .header("content-type", "application/octet-stream")
            .body(bytes.to_vec())
            .send()
            .await
            .expect("send request")
    }

    /// `QUERY` with the `type` in the query and the parameters in the JSON body.
    async fn query(&self, endpoint: &str, typing: &str, body: Value) -> reqwest::Response {
        self.send_json(query_method(), &format!("{endpoint}?type={typing}"), body)
            .await
    }

    /// `PUT type=stream`, where `path` travels in the query because the body is the content.
    async fn put_stream(&self, endpoint: &str, path: &str, bytes: &[u8]) -> reqwest::Response {
        let url = format!("{endpoint}?type=stream&path={}", encode_query_value(path));
        self.send_raw(Method::PUT, &url, bytes).await
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

fn query_method() -> Method {
    Method::from_bytes(b"QUERY").expect("QUERY is a valid method")
}

/// A path as an RFC 3986 query value: bytes outside the unreserved set are
/// percent-encoded, `+` excepted, since a literal `+` in a query is a sub-delim
/// and never a space.
fn encode_query_value(value: &str) -> String {
    let mut encoded = String::new();

    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'+' => {
                encoded.push(char::from(byte));
            }
            byte => encoded.push_str(&format!("%{byte:02X}")),
        }
    }

    encoded
}

/// Registers a workspace rooted at `root`, asserting the server accepted it.
async fn register(server: &Server, id: &str, root: &Path) {
    let body = json!({ "id": id, "root": root.to_string_lossy() });
    let response = server.send_json(Method::POST, "/workspaces", body).await;
    assert_eq!(
        response.status(),
        reqwest::StatusCode::CREATED,
        "register workspace {id}"
    );
}

mod mount {
    use super::*;

    #[tokio::test]
    async fn direct() {
        let home = TempDir::new("mount-direct");
        let server = Server::start(home.path()).await;
        let target = home.path().join("notes.txt");

        let response = server
            .put_stream("/fs", &target.to_string_lossy(), b"hello\n")
            .await;

        assert!(response.status().is_success(), "PUT /fs");
        assert_eq!(
            std::fs::read(&target).expect("read the written file"),
            b"hello\n"
        );
    }

    #[tokio::test]
    async fn workspace() {
        let home = TempDir::new("mount-workspace-home");
        let root = TempDir::new("mount-workspace-root");
        let server = Server::start(home.path()).await;
        register(&server, "w", root.path()).await;

        let response = server
            .put_stream("/workspaces/w/fs", "notes.txt", b"hello\n")
            .await;

        assert!(response.status().is_success(), "PUT /workspaces/w/fs");
        assert_eq!(
            std::fs::read(root.path().join("notes.txt")).expect("read the written file"),
            b"hello\n"
        );
    }

    #[tokio::test]
    async fn unknown_workspace() {
        let home = TempDir::new("mount-unknown");
        let server = Server::start(home.path()).await;

        let response = server
            .put_stream("/workspaces/missing/fs", "notes.txt", b"hello\n")
            .await;

        assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
    }
}

mod request {
    use super::*;

    #[tokio::test]
    async fn body() {
        let home = TempDir::new("request-body-home");
        let root = TempDir::new("request-body-root");
        let server = Server::start(home.path()).await;
        register(&server, "w", root.path()).await;
        std::fs::write(root.path().join("notes.txt"), b"hello\n").expect("seed the file");

        let carried = server
            .query(
                "/workspaces/w/fs",
                "metadata",
                json!({ "path": "notes.txt" }),
            )
            .await;
        assert!(carried.status().is_success(), "parameters in the JSON body");

        // The same parameters in the query string are not the request's parameters.
        let misplaced = server
            .send_json(
                query_method(),
                "/workspaces/w/fs?type=metadata&path=notes.txt",
                json!({}),
            )
            .await;
        assert!(
            !misplaced.status().is_success(),
            "parameters in the query string"
        );
    }
}

mod stream {
    use super::*;

    #[tokio::test]
    async fn write() {
        let home = TempDir::new("stream-write-home");
        let root = TempDir::new("stream-write-root");
        let server = Server::start(home.path()).await;
        register(&server, "w", root.path()).await;
        let bytes = b"\x00\xff\xfe binary\n";

        let response = server
            .put_stream("/workspaces/w/fs", "notes.bin", bytes)
            .await;

        assert!(response.status().is_success(), "PUT type=stream");
        assert_eq!(
            std::fs::read(root.path().join("notes.bin")).expect("read the written file"),
            bytes
        );
    }

    #[tokio::test]
    async fn read() {
        let home = TempDir::new("stream-read-home");
        let root = TempDir::new("stream-read-root");
        let server = Server::start(home.path()).await;
        register(&server, "w", root.path()).await;
        let bytes = b"\x00\xff\xfe binary\n";
        std::fs::write(root.path().join("notes.bin"), bytes).expect("seed the file");

        let response = server
            .query("/workspaces/w/fs", "stream", json!({ "path": "notes.bin" }))
            .await;

        assert_eq!(response.status(), reqwest::StatusCode::OK);
        assert_eq!(
            response.bytes().await.expect("read the stream").to_vec(),
            bytes
        );
    }

    #[tokio::test]
    async fn path_encoding() {
        let home = TempDir::new("stream-encoding-home");
        let root = TempDir::new("stream-encoding-root");
        let server = Server::start(home.path()).await;
        register(&server, "w", root.path()).await;

        for name in ["a+b 100%.txt", "笔记.md"] {
            let response = server
                .put_stream("/workspaces/w/fs", name, b"hello\n")
                .await;
            assert!(response.status().is_success(), "PUT {name}");
            assert_eq!(
                std::fs::read(root.path().join(name)).expect("read the written file"),
                b"hello\n"
            );
        }
    }
}
