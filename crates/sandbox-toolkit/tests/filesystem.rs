mod harness;

use reqwest::Method;
use serde_json::{Value, json};

use harness::{Dir, Server};

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

/// `QUERY` with the `type` in the query and the parameters in the JSON body.
async fn query(server: &Server, endpoint: &str, typing: &str, body: Value) -> reqwest::Response {
    let path = format!("{endpoint}?type={typing}");
    server.send_json(query_method(), &path, body).await
}

/// `PUT type=stream`, where `path` travels in the query because the body is the content.
async fn put_stream(
    server: &Server,
    endpoint: &str,
    path: &str,
    bytes: &[u8],
) -> reqwest::Response {
    let url = format!("{endpoint}?type=stream&path={}", encode_query_value(path));
    server.send_raw(Method::PUT, &url, bytes).await
}

mod mount {
    use super::*;

    #[tokio::test]
    async fn direct() {
        let home = Dir::new("mount-direct");
        let server = Server::start(&home).await;
        let target = home.path().join("notes.txt");

        let response = put_stream(&server, "/fs", &target.to_string_lossy(), b"hello\n").await;

        assert!(response.status().is_success(), "PUT /fs");
        assert_eq!(
            std::fs::read(&target).expect("read the written file"),
            b"hello\n"
        );
    }

    #[tokio::test]
    async fn workspace() {
        let home = Dir::new("mount-workspace-home");
        let root = Dir::new("mount-workspace-root");
        let server = Server::start(&home).await;
        server.register("w", root.path()).await;

        let response = put_stream(&server, "/workspaces/w/fs", "notes.txt", b"hello\n").await;

        assert!(response.status().is_success(), "PUT /workspaces/w/fs");
        assert_eq!(
            std::fs::read(root.path().join("notes.txt")).expect("read the written file"),
            b"hello\n"
        );
    }

    #[tokio::test]
    async fn unknown_workspace() {
        let home = Dir::new("mount-unknown");
        let server = Server::start(&home).await;

        let response = put_stream(&server, "/workspaces/missing/fs", "notes.txt", b"hello\n").await;

        assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
    }
}

mod request {
    use super::*;

    #[tokio::test]
    async fn body() {
        let home = Dir::new("request-body-home");
        let root = Dir::new("request-body-root");
        let server = Server::start(&home).await;
        server.register("w", root.path()).await;
        std::fs::write(root.path().join("notes.txt"), b"hello\n").expect("seed the file");

        let carried = query(
            &server,
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
        let home = Dir::new("stream-write-home");
        let root = Dir::new("stream-write-root");
        let server = Server::start(&home).await;
        server.register("w", root.path()).await;
        let bytes = b"\x00\xff\xfe binary\n";

        let response = put_stream(&server, "/workspaces/w/fs", "notes.bin", bytes).await;

        assert!(response.status().is_success(), "PUT type=stream");
        assert_eq!(
            std::fs::read(root.path().join("notes.bin")).expect("read the written file"),
            bytes
        );
    }

    #[tokio::test]
    async fn read() {
        let home = Dir::new("stream-read-home");
        let root = Dir::new("stream-read-root");
        let server = Server::start(&home).await;
        server.register("w", root.path()).await;
        let bytes = b"\x00\xff\xfe binary\n";
        std::fs::write(root.path().join("notes.bin"), bytes).expect("seed the file");

        let response =
            query(&server, "/workspaces/w/fs", "stream", json!({ "path": "notes.bin" })).await;

        assert_eq!(response.status(), reqwest::StatusCode::OK);
        assert_eq!(
            response.bytes().await.expect("read the stream").to_vec(),
            bytes
        );
    }

    #[tokio::test]
    async fn path_encoding() {
        let home = Dir::new("stream-encoding-home");
        let root = Dir::new("stream-encoding-root");
        let server = Server::start(&home).await;
        server.register("w", root.path()).await;

        for name in ["a+b 100%.txt", "笔记.md"] {
            let response =
                put_stream(&server, "/workspaces/w/fs", name, b"hello\n").await;
            assert!(response.status().is_success(), "PUT {name}");
            assert_eq!(
                std::fs::read(root.path().join(name)).expect("read the written file"),
                b"hello\n"
            );
        }
    }
}
