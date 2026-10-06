mod harness;

use std::path::{Path, PathBuf};

use reqwest::StatusCode;
use tokio::net::UnixStream;

use harness::{Dir, Server, TUS_VERSION, Upload};

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

mod mount {
    use super::*;

    #[tokio::test]
    async fn create() {
        let home = Dir::new("mount-create");
        let server = Server::start(&home).await;

        let response = harness::create(&server, 5).await;

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
        let upload = Upload::open(&server, 5).await;

        assert_eq!(
            upload.head().await.status(),
            StatusCode::OK,
            "HEAD {}",
            upload.url
        );
    }
}

mod proxy {
    use super::*;

    #[tokio::test]
    async fn resume() {
        let home = Dir::new("proxy-resume");
        let server = Server::start(&home).await;
        let upload = Upload::open(&server, 10).await;

        let first = upload.patch(0, b"abcd").await;
        assert_eq!(first.status(), StatusCode::NO_CONTENT, "PATCH at 0");
        let second = upload.patch(4, b"efghij").await;
        assert_eq!(second.status(), StatusCode::NO_CONTENT, "PATCH at 4");

        assert_eq!(upload.offset().await, 10);
    }

    #[tokio::test]
    async fn download() {
        let home = Dir::new("proxy-download");
        let server = Server::start(&home).await;
        let bytes = b"\x00\xff binary\n";
        let upload = server.upload(bytes).await;

        let response = upload.get().await;

        assert_eq!(response.status(), StatusCode::OK, "GET {}", upload.url);
        assert_eq!(
            response.bytes().await.expect("read the upload").to_vec(),
            bytes
        );
    }

    #[tokio::test]
    async fn terminate() {
        let home = Dir::new("proxy-terminate");
        let server = Server::start(&home).await;
        let upload = server.upload(b"abcd").await;

        let response = upload.delete().await;

        assert_eq!(
            response.status(),
            StatusCode::NO_CONTENT,
            "DELETE {}",
            upload.url
        );
        assert_eq!(upload.head().await.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn status() {
        let home = Dir::new("proxy-status");
        let server = Server::start(&home).await;

        let unknown = reqwest::Client::new()
            .head(server.url("/tus/missing"))
            .header("tus-resumable", TUS_VERSION)
            .send()
            .await
            .expect("HEAD an unknown upload");
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
        let upload = Upload::open(&server, bytes.len()).await;

        let response = upload.patch(0, &bytes).await;

        assert_eq!(response.status(), StatusCode::NO_CONTENT, "a 4 MiB PATCH");
        assert_eq!(upload.offset().await, bytes.len());
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
        assert_eq!(
            harness::create(&server, 0).await.status(),
            StatusCode::CREATED
        );
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

        assert_eq!(
            harness::create(&server, 0).await.status(),
            StatusCode::BAD_GATEWAY
        );
    }
}
