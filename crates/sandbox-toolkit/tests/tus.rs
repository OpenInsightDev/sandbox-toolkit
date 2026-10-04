mod harness;

use std::path::{Path, PathBuf};

use reqwest::StatusCode;
use tokio::net::UnixStream;

use harness::{Dir, Server};

/// The protocol version every tus request carries.
const TUS_VERSION: &str = "1.0.0";

/// Opens an upload of `length` bytes, the way a tus client does.
async fn open_upload(server: &Server, length: usize) -> reqwest::Response {
    reqwest::Client::new()
        .post(server.url("/tus"))
        .header("tus-resumable", TUS_VERSION)
        .header("upload-length", length.to_string())
        .send()
        .await
        .expect("POST /tus")
}

/// Appends `bytes` at `offset`, the way a tus client resumes one.
async fn patch(url: &str, offset: usize, bytes: &[u8]) -> reqwest::Response {
    reqwest::Client::new()
        .patch(url)
        .header("tus-resumable", TUS_VERSION)
        .header("upload-offset", offset.to_string())
        .header("content-type", "application/offset+octet-stream")
        .body(bytes.to_vec())
        .send()
        .await
        .expect("PATCH an upload")
}

async fn head(url: &str) -> reqwest::Response {
    reqwest::Client::new()
        .head(url)
        .header("tus-resumable", TUS_VERSION)
        .send()
        .await
        .expect("HEAD an upload")
}

async fn get(url: &str) -> reqwest::Response {
    reqwest::Client::new()
        .get(url)
        .send()
        .await
        .expect("GET an upload")
}

async fn delete(url: &str) -> reqwest::Response {
    reqwest::Client::new()
        .delete(url)
        .header("tus-resumable", TUS_VERSION)
        .send()
        .await
        .expect("DELETE an upload")
}

/// Where the sidecar listens: the system cache dir of `home`.
fn tus_socket(home: &Path) -> PathBuf {
    home.join(".cache/sandbox-toolkit/tus.sock")
}

/// The upload URL the server answered with, which the client is meant to use.
fn location(response: &reqwest::Response) -> String {
    response
        .headers()
        .get("location")
        .expect("a Location header")
        .to_str()
        .expect("a readable Location")
        .to_owned()
}

/// The bytes an upload holds, as the server reports them.
fn upload_offset(response: &reqwest::Response) -> usize {
    response
        .headers()
        .get("upload-offset")
        .expect("an Upload-Offset header")
        .to_str()
        .expect("a readable Upload-Offset")
        .parse()
        .expect("a numeric Upload-Offset")
}

mod mount {
    use super::*;

    #[tokio::test]
    async fn create() {
        let home = Dir::new("mount-create");
        let server = Server::start(&home).await;

        let response = open_upload(&server, 5).await;

        assert_eq!(response.status(), StatusCode::CREATED, "POST /tus");
        let location = location(&response);
        assert!(
            location.starts_with(&server.url("/tus/")),
            "the upload is reached at {location}"
        );
    }

    #[tokio::test]
    async fn targets() {
        let home = Dir::new("mount-targets");
        let server = Server::start(&home).await;
        let upload = location(&open_upload(&server, 5).await);

        let response = head(&upload).await;

        assert_eq!(response.status(), StatusCode::OK, "HEAD {upload}");
    }
}

mod proxy {
    use super::*;

    #[tokio::test]
    async fn resume() {
        let home = Dir::new("proxy-resume");
        let server = Server::start(&home).await;
        let upload = location(&open_upload(&server, 10).await);

        let first = patch(&upload, 0, b"abcd").await;
        assert_eq!(first.status(), StatusCode::NO_CONTENT, "PATCH at 0");
        let second = patch(&upload, 4, b"efghij").await;
        assert_eq!(second.status(), StatusCode::NO_CONTENT, "PATCH at 4");

        assert_eq!(upload_offset(&head(&upload).await), 10);
    }

    #[tokio::test]
    async fn download() {
        let home = Dir::new("proxy-download");
        let server = Server::start(&home).await;
        let bytes = b"\x00\xff binary\n";
        let upload = location(&open_upload(&server, bytes.len()).await);
        patch(&upload, 0, bytes).await;

        let response = get(&upload).await;

        assert_eq!(response.status(), StatusCode::OK, "GET {upload}");
        assert_eq!(
            response.bytes().await.expect("read the upload").to_vec(),
            bytes
        );
    }

    #[tokio::test]
    async fn terminate() {
        let home = Dir::new("proxy-terminate");
        let server = Server::start(&home).await;
        let upload = location(&open_upload(&server, 4).await);
        patch(&upload, 0, b"abcd").await;

        let response = delete(&upload).await;

        assert_eq!(response.status(), StatusCode::NO_CONTENT, "DELETE {upload}");
        assert_eq!(head(&upload).await.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn status() {
        let home = Dir::new("proxy-status");
        let server = Server::start(&home).await;

        let unknown = head(&server.url("/tus/missing")).await;
        assert_eq!(unknown.status(), StatusCode::NOT_FOUND, "an unknown upload");

        let incomplete = reqwest::Client::new()
            .post(server.url("/tus"))
            .header("upload-length", "1")
            .send()
            .await
            .expect("POST /tus");
        assert_eq!(
            incomplete.status(),
            StatusCode::PRECONDITION_FAILED,
            "a request without Tus-Resumable"
        );
    }

    #[tokio::test]
    async fn unlimited() {
        let home = Dir::new("proxy-unlimited");
        let server = Server::start(&home).await;
        let bytes = vec![b'x'; 4 * 1024 * 1024];
        let upload = location(&open_upload(&server, bytes.len()).await);

        let response = patch(&upload, 0, &bytes).await;

        assert_eq!(response.status(), StatusCode::NO_CONTENT, "a 4 MiB PATCH");
        assert_eq!(upload_offset(&head(&upload).await), bytes.len());
    }
}

mod process {
    use super::*;

    #[tokio::test]
    async fn ready() {
        let home = Dir::new("process-ready");
        let server = Server::start(&home).await;

        // The service answers only once the sidecar it started takes requests.
        assert!(UnixStream::connect(tus_socket(home.path())).await.is_ok());
        assert_eq!(open_upload(&server, 0).await.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn exits() {
        let home = Dir::new("process-exits");
        let mut server = Server::start(&home).await;
        let socket = tus_socket(home.path());
        assert!(UnixStream::connect(&socket).await.is_ok());

        server.terminate().await;

        assert!(
            UnixStream::connect(&socket).await.is_err(),
            "the sidecar outlived the service"
        );
    }

    #[tokio::test]
    async fn unreachable() {
        let home = Dir::new("process-unreachable");
        let server = Server::start(&home).await;

        // Only the service itself can take its sidecar away.
        std::fs::remove_file(tus_socket(home.path())).expect("remove the socket");

        assert_eq!(open_upload(&server, 0).await.status(), StatusCode::BAD_GATEWAY);
    }
}
