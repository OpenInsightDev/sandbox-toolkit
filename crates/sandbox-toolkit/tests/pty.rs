use std::fs::File;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use http_body_util::Empty;
use hyper::body::Bytes;
use hyper::client::conn::http2;
use hyper::{Method, Request, StatusCode};
use hyper_util::rt::{TokioExecutor, TokioIo};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::Role;

const STDIN: u8 = 0;
const STDOUT: u8 = 1;
const STDERR: u8 = 2;
const ERROR: u8 = 3;
const RESIZE: u8 = 4;

const MAX_PAYLOAD_LEN: usize = 4 * 1024 * 1024;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let serial = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("sbxtkt-pty-{tag}-{}-{serial}", std::process::id()));
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

    fn authority(&self) -> String {
        self.base_url.trim_start_matches("http://").to_owned()
    }

    async fn post(&self, path: &str, body: Value) -> reqwest::Response {
        reqwest::Client::new()
            .post(self.url(path))
            .json(&body)
            .send()
            .await
            .expect("POST request")
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

fn channel_bytes(frames: &[(u8, Vec<u8>)], channel: u8) -> Vec<u8> {
    frames
        .iter()
        .filter(|(id, _)| *id == channel)
        .flat_map(|(_, payload)| payload.clone())
        .collect()
}

fn channel_contains(frames: &[(u8, Vec<u8>)], channel: u8, needle: &[u8]) -> bool {
    channel_bytes(frames, channel)
        .windows(needle.len())
        .any(|window| window == needle)
}

fn terminal_status(frames: &[(u8, Vec<u8>)]) -> Value {
    let statuses = frames.iter().filter(|(id, _)| *id == ERROR).count();
    assert_eq!(statuses, 1, "exactly one terminal frame");
    let (channel, payload) = frames.last().expect("a terminal frame");
    assert_eq!(*channel, ERROR, "the terminal frame comes last");
    serde_json::from_slice(payload).expect("decode terminal status")
}

/// A pty session's WebSocket, tunneled over HTTP/2 extended CONNECT (RFC 8441).
type Pty = WebSocketStream<TokioIo<hyper::upgrade::Upgraded>>;

async fn attach_pty(server: &Server, endpoint: &str) -> (StatusCode, Option<Pty>) {
    let authority = server.authority();
    let io = TokioIo::new(
        TcpStream::connect(&authority)
            .await
            .expect("connect the server"),
    );
    let (mut send_request, connection) = http2::Builder::new(TokioExecutor::new())
        .handshake(io)
        .await
        .expect("HTTP/2 handshake");
    tokio::spawn(async move {
        let _ = connection.await;
    });

    let request = Request::builder()
        .method(Method::CONNECT)
        .extension(hyper::ext::Protocol::from_static("websocket"))
        .uri(endpoint)
        .header("host", &authority)
        // RFC 8441 keeps the RFC 6455 handshake fields that HTTP/1.1 does not
        // already carry, `Sec-WebSocket-Version` among them.
        .header("sec-websocket-version", "13")
        .body(Empty::<Bytes>::new())
        .expect("build the attach request");

    let mut response = send_request
        .send_request(request)
        .await
        .expect("send the attach request");
    let status = response.status();

    if status != StatusCode::OK {
        return (status, None);
    }

    let upgraded = hyper::upgrade::on(&mut response)
        .await
        .expect("upgrade the attach request");
    let socket = WebSocketStream::from_raw_socket(TokioIo::new(upgraded), Role::Client, None).await;
    (status, Some(socket))
}

fn stdin_message(text: &str) -> Message {
    let mut message = vec![STDIN];
    message.extend_from_slice(text.as_bytes());
    Message::Binary(message.into())
}

async fn drain(socket: &mut Pty) -> Vec<(u8, Vec<u8>)> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut frames = Vec::new();

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining, socket.next()).await {
            Ok(Some(Ok(Message::Binary(bytes)))) => {
                let channel = bytes[0];
                frames.push((channel, bytes[1..].to_vec()));
                if channel == ERROR {
                    return frames;
                }
            }
            Ok(Some(Ok(Message::Close(_)))) | Ok(None) => return frames,
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(error))) => panic!("pty socket error: {error}"),
            Err(_) => panic!("timed out waiting for the pty stream"),
        }
    }
}

mod pty {
    use super::*;

    async fn create_session(server: &Server) -> Value {
        let response = server.post("/pty", json!({ "command": "sh" })).await;
        assert_eq!(response.status(), reqwest::StatusCode::CREATED, "POST /pty");
        response.json().await.expect("decode the pty session")
    }

    async fn attach_session(server: &Server) -> Pty {
        let session = create_session(server).await;
        let endpoint = session["endpoint"].as_str().expect("session endpoint");
        let (status, socket) = attach_pty(server, endpoint).await;
        assert_eq!(status, StatusCode::OK, "attach {endpoint}");
        socket.expect("attached socket")
    }

    #[tokio::test]
    async fn create() {
        let home = TempDir::new("create");
        let server = Server::start(home.path()).await;

        let session = create_session(&server).await;
        let id = session["id"].as_str().expect("session id");

        assert_eq!(session["endpoint"], format!("/pty/{id}"));
    }

    #[tokio::test]
    async fn unknown_session() {
        let home = TempDir::new("unknown-session");
        let server = Server::start(home.path()).await;

        let (status, _) = attach_pty(&server, "/pty/missing").await;

        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn attach_once() {
        let home = TempDir::new("attach-once");
        let server = Server::start(home.path()).await;
        let session = create_session(&server).await;
        let endpoint = session["endpoint"].as_str().expect("session endpoint");

        let (first, socket) = attach_pty(&server, endpoint).await;
        assert_eq!(first, StatusCode::OK);

        let (second, _) = attach_pty(&server, endpoint).await;
        assert_eq!(second, StatusCode::NOT_FOUND);

        drop(socket);
    }

    #[tokio::test]
    async fn attach() {
        let home = TempDir::new("attach");
        let server = Server::start(home.path()).await;
        let mut socket = attach_session(&server).await;

        // The session runs an interactive shell, whose output carries the echo
        // of this line; a value the shell computes is the part that shows it
        // ran.
        socket
            .send(stdin_message("echo $((40 + 2))\nexit\n"))
            .await
            .expect("send stdin");
        let frames = drain(&mut socket).await;

        assert!(
            channel_contains(&frames, STDOUT, b"42"),
            "the shell's output arrives on channel 1"
        );
        assert_eq!(terminal_status(&frames)["status"], "exited");
    }

    #[tokio::test]
    async fn exit() {
        let home = TempDir::new("exit");
        let server = Server::start(home.path()).await;
        let mut socket = attach_session(&server).await;

        socket
            .send(stdin_message("exit 4\n"))
            .await
            .expect("send stdin");
        let frames = drain(&mut socket).await;

        let status = terminal_status(&frames);
        assert_eq!(status["status"], "exited");
        assert_eq!(status["exit_code"], 4);
    }

    #[tokio::test]
    async fn channels() {
        let home = TempDir::new("channels");
        let server = Server::start(home.path()).await;
        let mut socket = attach_session(&server).await;

        socket
            .send(stdin_message("printf out; printf err 1>&2\nexit\n"))
            .await
            .expect("send stdin");
        let frames = drain(&mut socket).await;

        assert!(
            channel_bytes(&frames, STDOUT)
                .windows(3)
                .any(|word| word == b"err")
        );
        assert!(frames.iter().all(|(id, _)| *id != STDERR));
    }

    #[tokio::test]
    async fn resize() {
        let home = TempDir::new("resize");
        let server = Server::start(home.path()).await;
        let mut socket = attach_session(&server).await;

        // 40 rows and 120 columns, which the session then reports.
        let resize = Message::Binary(vec![RESIZE, 0, 40, 0, 120].into());
        socket.send(resize).await.expect("send resize");
        socket
            .send(stdin_message("stty size\nexit\n"))
            .await
            .expect("send stdin");
        let frames = drain(&mut socket).await;

        assert!(
            channel_contains(&frames, STDOUT, b"40 120"),
            "the session resized to the requested size"
        );
    }

    #[tokio::test]
    async fn close() {
        let home = TempDir::new("close");
        let server = Server::start(home.path()).await;
        let session = create_session(&server).await;
        let endpoint = session["endpoint"]
            .as_str()
            .expect("session endpoint")
            .to_owned();
        let (_, socket) = attach_pty(&server, &endpoint).await;
        let mut socket = socket.expect("attached socket");

        socket.send(Message::Close(None)).await.expect("send close");

        // Output the shell already produced may still arrive before the close.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let closed = tokio::time::timeout_at(deadline, socket.next())
                .await
                .expect("the session closes");
            match closed {
                Some(Ok(Message::Binary(_))) => {}
                Some(Ok(Message::Close(_))) | None => break,
                other => panic!("unexpected message after close: {other:?}"),
            }
        }

        let (reclaimed, _) = attach_pty(&server, &endpoint).await;
        assert_eq!(reclaimed, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn reclaims() {
        let home = TempDir::new("reclaims");
        let server = Server::start(home.path()).await;
        let session = create_session(&server).await;
        let endpoint = session["endpoint"]
            .as_str()
            .expect("session endpoint")
            .to_owned();
        let (_, socket) = attach_pty(&server, &endpoint).await;
        drop(socket);

        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let (status, _) = attach_pty(&server, &endpoint).await;
            if status == StatusCode::NOT_FOUND {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the session was not reclaimed"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    #[tokio::test]
    #[ignore = "receiver-side rule: the client never sends an oversized message"]
    async fn message_limit() {
        let home = TempDir::new("message-limit");
        let server = Server::start(home.path()).await;
        let mut socket = attach_session(&server).await;

        let mut message = vec![STDIN];
        message.resize(MAX_PAYLOAD_LEN + 2, b'a');
        socket.send(Message::Binary(message.into())).await.ok();
    }
}
