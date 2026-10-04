mod harness;

use std::time::Duration;

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

use harness::{
    Dir, ERROR, MAX_PAYLOAD_LEN, RESIZE, STDERR, STDIN, STDOUT, Server, channel_bytes,
    terminal_status,
};

fn channel_contains(frames: &[(u8, Vec<u8>)], channel: u8, needle: &[u8]) -> bool {
    channel_bytes(frames, channel)
        .windows(needle.len())
        .any(|window| window == needle)
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
        let home = Dir::new("create");
        let server = Server::start(&home).await;

        let session = create_session(&server).await;
        let id = session["id"].as_str().expect("session id");

        assert_eq!(session["endpoint"], format!("/pty/{id}"));
    }

    #[tokio::test]
    async fn unknown_session() {
        let home = Dir::new("unknown-session");
        let server = Server::start(&home).await;

        let (status, _) = attach_pty(&server, "/pty/missing").await;

        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn attach_once() {
        let home = Dir::new("attach-once");
        let server = Server::start(&home).await;
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
        let home = Dir::new("attach");
        let server = Server::start(&home).await;
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
        let home = Dir::new("exit");
        let server = Server::start(&home).await;
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
        let home = Dir::new("channels");
        let server = Server::start(&home).await;
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
        let home = Dir::new("resize");
        let server = Server::start(&home).await;
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
        let home = Dir::new("close");
        let server = Server::start(&home).await;
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
        let home = Dir::new("reclaims");
        let server = Server::start(&home).await;
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
        let home = Dir::new("message-limit");
        let server = Server::start(&home).await;
        let mut socket = attach_session(&server).await;

        let mut message = vec![STDIN];
        message.resize(MAX_PAYLOAD_LEN + 2, b'a');
        socket.send(Message::Binary(message.into())).await.ok();
    }
}
