//! End-to-end tests driving the router with the real `FileStore` and
//! `MemoryLocker`, ported from tusd's `pkg/handler/*_test.go`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::Router;
use axum::body::Body;
use axum::http::header::{self, HeaderMap};
use axum::http::{Method, Request, StatusCode};
use tower::ServiceExt;

use tus::{Config, FileLocker, FileStore, Handler, MemoryLocker, StoreComposer, router};

const TUS: &str = "Tus-Resumable";
const VERSION: &str = "1.0.0";
const BASE: &str = "/files";
const OFFSET_CONTENT_TYPE: &str = "application/offset+octet-stream";

struct Harness {
    app: Router,
    root: PathBuf,
    _dir: TempDir,
}

fn base_config() -> Config {
    Config {
        base_path: BASE.to_owned(),
        ..Config::default()
    }
}

fn harness(config: Config) -> Harness {
    let dir = TempDir::new();
    let store = Arc::new(FileStore::new(dir.path()));
    let locker = Arc::new(MemoryLocker::new());
    let composer = locker.use_in(store.use_in(StoreComposer::new()));
    let handler = Arc::new(Handler::new(config, composer).expect("building the handler failed"));

    Harness {
        app: router(handler),
        root: dir.path().to_owned(),
        _dir: dir,
    }
}

fn filelocker_harness(config: Config) -> Harness {
    let dir = TempDir::new();
    let store = Arc::new(FileStore::new(dir.path()));
    let locker = Arc::new(FileLocker::new(dir.path()));
    let composer = locker.use_in(store.use_in(StoreComposer::new()));
    let handler = Arc::new(Handler::new(config, composer).expect("building the handler failed"));

    Harness {
        app: router(handler),
        root: dir.path().to_owned(),
        _dir: dir,
    }
}

struct TestResponse {
    status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
}

impl TestResponse {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }

    fn body_str(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    fn expect_status(&self, expected: StatusCode) -> &Self {
        assert_eq!(
            self.status,
            expected,
            "unexpected status; body: {}",
            self.body_str()
        );
        self
    }
}

async fn send(app: &Router, request: Request<Body>) -> TestResponse {
    let response = app
        .clone()
        .oneshot(request)
        .await
        .expect("the router request failed");

    let status = response.status();
    let headers = response.headers().clone();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("reading the response body failed");

    TestResponse {
        status,
        headers,
        body: body.to_vec(),
    }
}

fn builder(method: Method, path: &str) -> axum::http::request::Builder {
    Request::builder().method(method).uri(path)
}

fn id_from(response: &TestResponse) -> String {
    let location = response
        .header(header::LOCATION.as_str())
        .expect("no Location");
    location
        .strip_prefix(&format!("{BASE}/"))
        .expect("unexpected Location")
        .to_owned()
}

/// Creates an upload via `POST` and returns its ID.
async fn create(app: &Router, length: u64, metadata: Option<&str>) -> String {
    let mut request = builder(Method::POST, BASE)
        .header(TUS, VERSION)
        .header("Upload-Length", length.to_string());

    if let Some(metadata) = metadata {
        request = request.header("Upload-Metadata", metadata);
    }

    let response = send(app, request.body(Body::empty()).unwrap()).await;
    response.expect_status(StatusCode::CREATED);
    id_from(&response)
}

/// Creates a partial upload (`Upload-Concat: partial`).
async fn create_partial(app: &Router, length: u64) -> String {
    let response = send(
        app,
        builder(Method::POST, BASE)
            .header(TUS, VERSION)
            .header("Upload-Length", length.to_string())
            .header("Upload-Concat", "partial")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    response.expect_status(StatusCode::CREATED);
    id_from(&response)
}

/// Sends a `PATCH` with an explicit `Content-Length` so the handler can reject
/// oversized chunks up front.
async fn patch(app: &Router, id: &str, offset: u64, body: &[u8]) -> TestResponse {
    send(
        app,
        builder(Method::PATCH, &format!("{BASE}/{id}"))
            .header(TUS, VERSION)
            .header(header::CONTENT_TYPE, OFFSET_CONTENT_TYPE)
            .header("Upload-Offset", offset.to_string())
            .header(header::CONTENT_LENGTH, body.len().to_string())
            .body(Body::from(body.to_vec()))
            .unwrap(),
    )
    .await
}

async fn head(app: &Router, id: &str) -> TestResponse {
    send(
        app,
        builder(Method::HEAD, &format!("{BASE}/{id}"))
            .header(TUS, VERSION)
            .body(Body::empty())
            .unwrap(),
    )
    .await
}

// region: OPTIONS

#[tokio::test]
async fn options_discovery() {
    let harness = harness(Config {
        max_size: 400,
        ..base_config()
    });

    let response = send(
        &harness.app,
        builder(Method::OPTIONS, BASE).body(Body::empty()).unwrap(),
    )
    .await;

    response.expect_status(StatusCode::OK);
    assert_eq!(
        response.header("Tus-Extension"),
        Some("creation,creation-with-upload,termination,concatenation")
    );
    assert_eq!(response.header("Tus-Version"), Some(VERSION));
    assert_eq!(response.header(TUS), Some(VERSION));
    assert_eq!(response.header("Tus-Max-Size"), Some("400"));
}

#[tokio::test]
async fn options_discovery_disable_termination() {
    let harness = harness(Config {
        disable_termination: true,
        ..base_config()
    });

    let response = send(
        &harness.app,
        builder(Method::OPTIONS, BASE).body(Body::empty()).unwrap(),
    )
    .await;

    response.expect_status(StatusCode::OK);
    assert_eq!(
        response.header("Tus-Extension"),
        Some("creation,creation-with-upload,concatenation")
    );
}

#[tokio::test]
async fn options_discovery_disable_concatenation() {
    let harness = harness(Config {
        disable_concatenation: true,
        ..base_config()
    });

    let response = send(
        &harness.app,
        builder(Method::OPTIONS, BASE).body(Body::empty()).unwrap(),
    )
    .await;

    response.expect_status(StatusCode::OK);
    assert_eq!(
        response.header("Tus-Extension"),
        Some("creation,creation-with-upload,termination")
    );
}

#[tokio::test]
async fn invalid_version_returns_412() {
    let harness = harness(base_config());

    let response = send(
        &harness.app,
        builder(Method::POST, BASE)
            .header(TUS, "foo")
            .header("Upload-Length", "300")
            .body(Body::empty())
            .unwrap(),
    )
    .await;

    response.expect_status(StatusCode::PRECONDITION_FAILED);
    assert_eq!(response.header("Tus-Version"), Some(VERSION));
    assert_eq!(response.header(TUS), Some(VERSION));
}

// endregion

// region: POST creation

#[tokio::test]
async fn create_upload() {
    let harness = harness(base_config());

    let response = send(
        &harness.app,
        builder(Method::POST, BASE)
            .header(TUS, VERSION)
            .header("Upload-Length", "300")
            // Invalid base64 values are ignored.
            .header(
                "Upload-Metadata",
                "foo aGVsbG8=, bar d29ybGQ=, hah INVALID, empty",
            )
            .body(Body::empty())
            .unwrap(),
    )
    .await;

    response.expect_status(StatusCode::CREATED);
    assert!(response.header(header::LOCATION.as_str()).is_some());
    assert_eq!(response.header(TUS), Some(VERSION));
    assert_eq!(response.header("X-Content-Type-Options"), Some("nosniff"));
}

#[tokio::test]
async fn create_empty_upload() {
    let harness = harness(base_config());
    let id = create(&harness.app, 0, None).await;

    let response = head(&harness.app, &id).await;
    response.expect_status(StatusCode::OK);
    assert_eq!(response.header("Upload-Offset"), Some("0"));
    assert_eq!(response.header("Upload-Length"), Some("0"));
}

#[tokio::test]
async fn create_exceeding_max_size() {
    let harness = harness(Config {
        max_size: 400,
        ..base_config()
    });

    let response = send(
        &harness.app,
        builder(Method::POST, BASE)
            .header(TUS, VERSION)
            .header("Upload-Length", "500")
            .body(Body::empty())
            .unwrap(),
    )
    .await;

    response.expect_status(StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        response.body_str(),
        "ERR_MAX_SIZE_EXCEEDED: maximum size exceeded\n"
    );
}

#[tokio::test]
async fn create_invalid_upload_length() {
    let harness = harness(base_config());

    let response = send(
        &harness.app,
        builder(Method::POST, BASE)
            .header(TUS, VERSION)
            .header("Upload-Length", "-5")
            .body(Body::empty())
            .unwrap(),
    )
    .await;

    response.expect_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn create_defer_length_not_implemented() {
    let harness = harness(base_config());

    let response = send(
        &harness.app,
        builder(Method::POST, BASE)
            .header(TUS, VERSION)
            .header("Upload-Defer-Length", "1")
            .body(Body::empty())
            .unwrap(),
    )
    .await;

    response.expect_status(StatusCode::NOT_IMPLEMENTED);
}

#[tokio::test]
async fn create_invalid_defer_length() {
    let harness = harness(base_config());

    let response = send(
        &harness.app,
        builder(Method::POST, BASE)
            .header(TUS, VERSION)
            .header("Upload-Defer-Length", "bad")
            .body(Body::empty())
            .unwrap(),
    )
    .await;

    response.expect_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn create_missing_length() {
    let harness = harness(base_config());

    let response = send(
        &harness.app,
        builder(Method::POST, BASE)
            .header(TUS, VERSION)
            .body(Body::empty())
            .unwrap(),
    )
    .await;

    response.expect_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn create_with_upload() {
    let harness = harness(base_config());

    let response = send(
        &harness.app,
        builder(Method::POST, BASE)
            .header(TUS, VERSION)
            .header("Upload-Length", "300")
            .header(header::CONTENT_TYPE, OFFSET_CONTENT_TYPE)
            .header("Upload-Metadata", "foo aGVsbG8=, bar d29ybGQ=")
            .body(Body::from("hello"))
            .unwrap(),
    )
    .await;

    response.expect_status(StatusCode::CREATED);
    assert_eq!(response.header("Upload-Offset"), Some("5"));

    let id = id_from(&response);
    assert_eq!(std::fs::read(harness.root.join(&id)).unwrap(), b"hello");
}

#[tokio::test]
async fn create_with_upload_exceeding_size() {
    let harness = harness(base_config());

    let response = send(
        &harness.app,
        builder(Method::POST, BASE)
            .header(TUS, VERSION)
            .header("Upload-Length", "300")
            .header(header::CONTENT_TYPE, OFFSET_CONTENT_TYPE)
            .header(header::CONTENT_LENGTH, "400")
            .body(Body::from(vec![0u8; 400]))
            .unwrap(),
    )
    .await;

    response.expect_status(StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn create_with_incorrect_content_type() {
    let harness = harness(base_config());

    let response = send(
        &harness.app,
        builder(Method::POST, BASE)
            .header(TUS, VERSION)
            .header("Upload-Length", "300")
            .header(header::CONTENT_TYPE, "application/false")
            .body(Body::from("hello"))
            .unwrap(),
    )
    .await;

    response.expect_status(StatusCode::CREATED);
    assert_eq!(response.header("Upload-Offset"), None);
}

#[tokio::test]
async fn create_final_with_chunk_forbidden() {
    let harness = harness(base_config());

    let response = send(
        &harness.app,
        builder(Method::POST, BASE)
            .header(TUS, VERSION)
            .header("Upload-Length", "300")
            .header(header::CONTENT_TYPE, OFFSET_CONTENT_TYPE)
            .header("Upload-Concat", "final;/files/a /files/b")
            .body(Body::from("hello"))
            .unwrap(),
    )
    .await;

    response.expect_status(StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn create_invalid_concat_header() {
    let harness = harness(base_config());

    let response = send(
        &harness.app,
        builder(Method::POST, BASE)
            .header(TUS, VERSION)
            .header("Upload-Concat", "final;")
            .body(Body::empty())
            .unwrap(),
    )
    .await;

    response.expect_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn create_disable_concatenation() {
    let harness = harness(Config {
        disable_concatenation: true,
        ..base_config()
    });

    let response = send(
        &harness.app,
        builder(Method::POST, BASE)
            .header(TUS, VERSION)
            .header("Upload-Concat", "final;/files/a /files/b")
            .body(Body::empty())
            .unwrap(),
    )
    .await;

    response.expect_status(StatusCode::BAD_REQUEST);
    assert_eq!(
        response.body_str(),
        "ERR_CONCATENATION_UNSUPPORTED: Upload-Concat header is not supported by server\n"
    );
}

// endregion

// region: HEAD

#[tokio::test]
async fn head_upload_status() {
    let harness = harness(base_config());
    let id = create(&harness.app, 300, Some("name bHVucmpzLnBuZw==, empty")).await;
    let _ = patch(&harness.app, &id, 0, b"hello").await;

    let response = head(&harness.app, &id).await;
    response.expect_status(StatusCode::OK);
    assert_eq!(response.header("Upload-Offset"), Some("5"));
    assert_eq!(response.header("Upload-Length"), Some("300"));
    assert_eq!(
        response.header(header::CONTENT_LENGTH.as_str()),
        Some("300")
    );
    assert_eq!(
        response.header(header::CACHE_CONTROL.as_str()),
        Some("no-store")
    );
    assert_eq!(
        response.header("Upload-Metadata"),
        Some("empty ,name bHVucmpzLnBuZw==")
    );
}

#[tokio::test]
async fn head_upload_not_found() {
    let harness = harness(base_config());

    let response = head(&harness.app, "missing").await;
    response.expect_status(StatusCode::NOT_FOUND);
    assert!(response.body.is_empty(), "HEAD errors must not have a body");
}

// endregion

// region: PATCH

#[tokio::test]
async fn patch_upload_chunk() {
    let harness = harness(base_config());
    let id = create(&harness.app, 10, None).await;

    let first = patch(&harness.app, &id, 0, b"hello").await;
    first.expect_status(StatusCode::NO_CONTENT);
    assert_eq!(first.header("Upload-Offset"), Some("5"));

    let second = patch(&harness.app, &id, 5, b"world").await;
    second.expect_status(StatusCode::NO_CONTENT);
    assert_eq!(second.header("Upload-Offset"), Some("10"));
    assert_eq!(
        std::fs::read(harness.root.join(&id)).unwrap(),
        b"helloworld"
    );
}

#[tokio::test]
async fn patch_method_override() {
    let harness = harness(base_config());
    let id = create(&harness.app, 10, None).await;

    let response = send(
        &harness.app,
        builder(Method::POST, &format!("{BASE}/{id}"))
            .header(TUS, VERSION)
            .header("X-HTTP-Method-Override", "PATCH")
            .header(header::CONTENT_TYPE, OFFSET_CONTENT_TYPE)
            .header("Upload-Offset", "0")
            .body(Body::from("hello"))
            .unwrap(),
    )
    .await;

    response.expect_status(StatusCode::NO_CONTENT);
    assert_eq!(response.header("Upload-Offset"), Some("5"));
}

#[tokio::test]
async fn patch_finished_upload_is_a_no_op() {
    let harness = harness(base_config());
    let id = create(&harness.app, 5, None).await;
    patch(&harness.app, &id, 0, b"hello").await;

    let response = patch(&harness.app, &id, 5, b"ignored").await;
    response.expect_status(StatusCode::NO_CONTENT);
    assert_eq!(response.header("Upload-Offset"), Some("5"));
    assert_eq!(std::fs::read(harness.root.join(&id)).unwrap(), b"hello");
}

#[tokio::test]
async fn patch_not_found() {
    let harness = harness(base_config());

    let response = patch(&harness.app, "missing", 0, b"hello").await;
    response.expect_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn patch_mismatched_offset() {
    let harness = harness(base_config());
    let id = create(&harness.app, 10, None).await;

    let response = patch(&harness.app, &id, 4, b"hello").await;
    response.expect_status(StatusCode::CONFLICT);
}

#[tokio::test]
async fn patch_exceeding_size() {
    let harness = harness(base_config());
    let id = create(&harness.app, 10, None).await;

    let response = patch(&harness.app, &id, 0, &[b'a'; 20]).await;
    response.expect_status(StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn patch_invalid_content_type() {
    let harness = harness(base_config());
    let id = create(&harness.app, 10, None).await;

    let response = send(
        &harness.app,
        builder(Method::PATCH, &format!("{BASE}/{id}"))
            .header(TUS, VERSION)
            .header(header::CONTENT_TYPE, "application/fail")
            .header("Upload-Offset", "0")
            .body(Body::from("hello"))
            .unwrap(),
    )
    .await;

    response.expect_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn patch_invalid_offset() {
    let harness = harness(base_config());
    let id = create(&harness.app, 10, None).await;

    let response = send(
        &harness.app,
        builder(Method::PATCH, &format!("{BASE}/{id}"))
            .header(TUS, VERSION)
            .header(header::CONTENT_TYPE, OFFSET_CONTENT_TYPE)
            .header("Upload-Offset", "-5")
            .body(Body::from("hello"))
            .unwrap(),
    )
    .await;

    response.expect_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn patch_overflow_without_length() {
    let harness = harness(base_config());
    let id = create(&harness.app, 20, None).await;
    patch(&harness.app, &id, 0, b"hello").await;

    // No Content-Length: the handler cannot reject up front, so it writes only
    // the remaining 15 bytes and then reports the overflow.
    let response = send(
        &harness.app,
        builder(Method::PATCH, &format!("{BASE}/{id}"))
            .header(TUS, VERSION)
            .header(header::CONTENT_TYPE, OFFSET_CONTENT_TYPE)
            .header("Upload-Offset", "5")
            .body(Body::from("hellothisismorethan15bytes"))
            .unwrap(),
    )
    .await;

    response.expect_status(StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        response.body_str(),
        "ERR_UPLOAD_SIZE_EXCEEDED: upload's size exceeded\n"
    );
    assert_eq!(
        std::fs::read(harness.root.join(&id)).unwrap(),
        b"hellohellothisismore"
    );
}

// endregion

// region: DELETE

#[tokio::test]
async fn terminate_upload() {
    let harness = harness(base_config());
    let id = create(&harness.app, 10, None).await;

    let response = send(
        &harness.app,
        builder(Method::DELETE, &format!("{BASE}/{id}"))
            .header(TUS, VERSION)
            .body(Body::empty())
            .unwrap(),
    )
    .await;

    response.expect_status(StatusCode::NO_CONTENT);
    assert!(!harness.root.join(&id).exists());
    assert!(!harness.root.join(format!("{id}.info")).exists());

    let response = head(&harness.app, &id).await;
    response.expect_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn terminate_disabled() {
    let harness = harness(Config {
        disable_termination: true,
        ..base_config()
    });
    let id = create(&harness.app, 10, None).await;

    let response = send(
        &harness.app,
        builder(Method::DELETE, &format!("{BASE}/{id}"))
            .header(TUS, VERSION)
            .body(Body::empty())
            .unwrap(),
    )
    .await;

    response.expect_status(StatusCode::NOT_IMPLEMENTED);
}

// endregion

// region: file locker

#[tokio::test]
async fn file_locker_end_to_end() {
    let harness = filelocker_harness(base_config());
    let id = create(&harness.app, 10, None).await;

    let response = patch(&harness.app, &id, 0, b"hello").await;
    response.expect_status(StatusCode::NO_CONTENT);

    let response = head(&harness.app, &id).await;
    response.expect_status(StatusCode::OK);
    assert_eq!(response.header("Upload-Offset"), Some("5"));

    let response = send(
        &harness.app,
        builder(Method::DELETE, &format!("{BASE}/{id}"))
            .header(TUS, VERSION)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    response.expect_status(StatusCode::NO_CONTENT);

    let response = head(&harness.app, &id).await;
    response.expect_status(StatusCode::NOT_FOUND);
}

// endregion

// region: concatenation

#[tokio::test]
async fn concat_partial_status() {
    let harness = harness(base_config());
    let id = create_partial(&harness.app, 5).await;

    let response = head(&harness.app, &id).await;
    response.expect_status(StatusCode::OK);
    assert_eq!(response.header("Upload-Concat"), Some("partial"));
}

#[tokio::test]
async fn concat_final() {
    let harness = harness(base_config());

    let a = create_partial(&harness.app, 5).await;
    patch(&harness.app, &a, 0, b"hello").await;
    let b = create_partial(&harness.app, 5).await;
    patch(&harness.app, &b, 0, b"world").await;

    let response = send(
        &harness.app,
        builder(Method::POST, BASE)
            .header(TUS, VERSION)
            // A space after `final;` is allowed for compatibility.
            .header("Upload-Concat", format!("final; {BASE}/{a} {BASE}/{b}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;

    response.expect_status(StatusCode::CREATED);
    let final_id = id_from(&response);

    // The final upload is the concatenation of the partial binaries.
    assert_eq!(
        std::fs::read(harness.root.join(&final_id)).unwrap(),
        b"helloworld"
    );

    let response = head(&harness.app, &final_id).await;
    response.expect_status(StatusCode::OK);
    assert_eq!(response.header("Upload-Offset"), Some("10"));
    assert_eq!(response.header("Upload-Length"), Some("10"));
    let expected_concat = format!("final;{BASE}/{a} {BASE}/{b}");
    assert_eq!(
        response.header("Upload-Concat"),
        Some(expected_concat.as_str())
    );
}

#[tokio::test]
async fn concat_unfinished_partial() {
    let harness = harness(base_config());

    let a = create_partial(&harness.app, 5).await;
    // Deliberately not patched: offset 0 != size 5.

    let response = send(
        &harness.app,
        builder(Method::POST, BASE)
            .header(TUS, VERSION)
            .header("Upload-Concat", format!("final;{BASE}/{a}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;

    response.expect_status(StatusCode::BAD_REQUEST);
    assert_eq!(
        response.body_str(),
        "ERR_UPLOAD_NOT_FINISHED: one of the partial uploads is not finished\n"
    );
}

#[tokio::test]
async fn concat_exceeding_max_size() {
    let harness = harness(Config {
        max_size: 100,
        ..base_config()
    });

    let a = create_partial(&harness.app, 60).await;
    patch(&harness.app, &a, 0, &[0u8; 60]).await;
    let b = create_partial(&harness.app, 60).await;
    patch(&harness.app, &b, 0, &[0u8; 60]).await;

    let response = send(
        &harness.app,
        builder(Method::POST, BASE)
            .header(TUS, VERSION)
            .header("Upload-Concat", format!("final;{BASE}/{a} {BASE}/{b}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;

    response.expect_status(StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn patch_final_upload_forbidden() {
    let harness = harness(base_config());

    let a = create_partial(&harness.app, 5).await;
    patch(&harness.app, &a, 0, b"hello").await;
    let b = create_partial(&harness.app, 5).await;
    patch(&harness.app, &b, 0, b"world").await;

    let response = send(
        &harness.app,
        builder(Method::POST, BASE)
            .header(TUS, VERSION)
            .header("Upload-Concat", format!("final;{BASE}/{a} {BASE}/{b}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    response.expect_status(StatusCode::CREATED);
    let final_id = id_from(&response);

    let response = patch(&harness.app, &final_id, 0, b"nope").await;
    response.expect_status(StatusCode::FORBIDDEN);
}

// endregion

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);

        let path = std::env::temp_dir().join(format!(
            "tus-e2e-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir_all(&path).expect("creating the temp dir failed");

        Self(path)
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
