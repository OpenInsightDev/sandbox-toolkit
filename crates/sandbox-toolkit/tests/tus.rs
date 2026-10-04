use std::fs::File;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use reqwest::StatusCode;
use tokio::net::UnixStream;

/// The protocol version every tus request carries.
const TUS_VERSION: &str = "1.0.0";

/// A directory removed when the test ends.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let serial = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("sbxtkt-tus-{tag}-{}-{serial}", std::process::id()));
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

    /// Creates an upload of `length` bytes, the way a tus client opens one.
    async fn create(&self, length: usize) -> reqwest::Response {
        reqwest::Client::new()
            .post(self.url("/tus"))
            .header("tus-resumable", TUS_VERSION)
            .header("upload-length", length.to_string())
            .send()
            .await
            .expect("POST /tus")
    }

    /// Appends `bytes` at `offset`, the way a tus client resumes one.
    async fn patch(&self, url: &str, offset: usize, bytes: &[u8]) -> reqwest::Response {
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

    async fn head(&self, url: &str) -> reqwest::Response {
        reqwest::Client::new()
            .head(url)
            .header("tus-resumable", TUS_VERSION)
            .send()
            .await
            .expect("HEAD an upload")
    }

    async fn get(&self, url: &str) -> reqwest::Response {
        reqwest::Client::new()
            .get(url)
            .send()
            .await
            .expect("GET an upload")
    }

    async fn delete(&self, url: &str) -> reqwest::Response {
        reqwest::Client::new()
            .delete(url)
            .header("tus-resumable", TUS_VERSION)
            .send()
            .await
            .expect("DELETE an upload")
    }

    /// Asks the service to stop, the way the container it runs in does.
    async fn terminate(&mut self) {
        // SAFETY: the pid names this child.
        unsafe { libc::kill(self.child.id() as libc::pid_t, libc::SIGTERM) };

        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if self.child.try_wait().expect("poll server").is_some() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("the server did not stop on SIGTERM");
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
        // Asking the service to stop is what stops its sidecar; a kill would leave
        // the child behind.
        // SAFETY: the pid names this child.
        unsafe { libc::kill(self.child.id() as libc::pid_t, libc::SIGTERM) };

        let deadline = Instant::now() + Duration::from_secs(10);
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

fn free_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .expect("reserve a port")
        .local_addr()
        .expect("read the reserved address")
        .port()
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
        let home = TempDir::new("mount-create");
        let server = Server::start(home.path()).await;

        let response = server.create(5).await;

        assert_eq!(response.status(), StatusCode::CREATED, "POST /tus");
        let location = location(&response);
        assert!(
            location.starts_with(&format!("{}/tus/", server.base_url)),
            "the upload is reached at {location}"
        );
    }

    #[tokio::test]
    async fn targets() {
        let home = TempDir::new("mount-targets");
        let server = Server::start(home.path()).await;
        let upload = location(&server.create(5).await);

        let response = server.head(&upload).await;

        assert_eq!(response.status(), StatusCode::OK, "HEAD {upload}");
    }
}

mod proxy {
    use super::*;

    #[tokio::test]
    async fn resume() {
        let home = TempDir::new("proxy-resume");
        let server = Server::start(home.path()).await;
        let upload = location(&server.create(10).await);

        let first = server.patch(&upload, 0, b"abcd").await;
        assert_eq!(first.status(), StatusCode::NO_CONTENT, "PATCH at 0");
        let second = server.patch(&upload, 4, b"efghij").await;
        assert_eq!(second.status(), StatusCode::NO_CONTENT, "PATCH at 4");

        assert_eq!(upload_offset(&server.head(&upload).await), 10);
    }

    #[tokio::test]
    async fn download() {
        let home = TempDir::new("proxy-download");
        let server = Server::start(home.path()).await;
        let bytes = b"\x00\xff binary\n";
        let upload = location(&server.create(bytes.len()).await);
        server.patch(&upload, 0, bytes).await;

        let response = server.get(&upload).await;

        assert_eq!(response.status(), StatusCode::OK, "GET {upload}");
        assert_eq!(
            response.bytes().await.expect("read the upload").to_vec(),
            bytes
        );
    }

    #[tokio::test]
    async fn terminate() {
        let home = TempDir::new("proxy-terminate");
        let server = Server::start(home.path()).await;
        let upload = location(&server.create(4).await);
        server.patch(&upload, 0, b"abcd").await;

        let response = server.delete(&upload).await;

        assert_eq!(response.status(), StatusCode::NO_CONTENT, "DELETE {upload}");
        assert_eq!(server.head(&upload).await.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn status() {
        let home = TempDir::new("proxy-status");
        let server = Server::start(home.path()).await;

        let unknown = server.head(&server.url("/tus/missing")).await;
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
        let home = TempDir::new("proxy-unlimited");
        let server = Server::start(home.path()).await;
        let bytes = vec![b'x'; 4 * 1024 * 1024];
        let upload = location(&server.create(bytes.len()).await);

        let response = server.patch(&upload, 0, &bytes).await;

        assert_eq!(response.status(), StatusCode::NO_CONTENT, "a 4 MiB PATCH");
        assert_eq!(upload_offset(&server.head(&upload).await), bytes.len());
    }
}

mod process {
    use super::*;

    #[tokio::test]
    async fn ready() {
        let home = TempDir::new("process-ready");
        let server = Server::start(home.path()).await;

        // The service answers only once the sidecar it started takes requests.
        assert!(UnixStream::connect(tus_socket(home.path())).await.is_ok());
        assert_eq!(server.create(0).await.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn exits() {
        let home = TempDir::new("process-exits");
        let mut server = Server::start(home.path()).await;
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
        let home = TempDir::new("process-unreachable");
        let server = Server::start(home.path()).await;

        // Only the service itself can take its sidecar away.
        std::fs::remove_file(tus_socket(home.path())).expect("remove the socket");

        assert_eq!(server.create(0).await.status(), StatusCode::BAD_GATEWAY);
    }
}
