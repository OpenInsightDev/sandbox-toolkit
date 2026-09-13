mod common;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use tower::ServiceExt;

use crate::common::make_test_router;

fn request(method: Method, path: &str, range: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(value) = range {
        builder = builder.header("range", value);
    }
    builder.body(Body::empty()).unwrap()
}

#[tokio::test]
async fn crlf_lf_cr_and_unterminated_bytes_are_preserved() {
    let dir = tempfile::TempDir::new().unwrap();
    let data = b"one\r\ntwo\rthree\nfour";
    std::fs::write(dir.path().join("lines.txt"), data).unwrap();
    let app = make_test_router(dir.path(), sbx::AuthState::new());

    let response = app
        .oneshot(request(Method::GET, "/lines.txt", Some("lines=1-4")))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(response.headers()["content-range"], "lines 1-4/4");
    assert_eq!(response.headers()["content-length"], data.len().to_string());
    assert_eq!(
        axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
        &data[..]
    );
}

#[tokio::test]
async fn empty_file_returns_416_with_zero_total() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("empty.txt"), []).unwrap();
    let app = make_test_router(dir.path(), sbx::AuthState::new());
    let response = app
        .oneshot(request(Method::GET, "/empty.txt", Some("lines=1-")))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(response.headers()["content-range"], "lines */0");
    assert!(
        axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn closed_open_ended_and_clipped_ranges_return_206() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("lines.txt"), b"one\ntwo\nthree").unwrap();
    let app = make_test_router(dir.path(), sbx::AuthState::new());

    for (range, expected_range, expected_body) in [
        ("lines=1-1", "lines 1-1/3", &b"one\n"[..]),
        ("lines=2-", "lines 2-3/3", &b"two\nthree"[..]),
        ("lines=2-999", "lines 2-3/3", &b"two\nthree"[..]),
    ] {
        let response = app
            .clone()
            .oneshot(request(Method::GET, "/lines.txt", Some(range)))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(response.headers()["content-range"], expected_range);
        assert_eq!(
            axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap(),
            expected_body
        );
    }
}

#[tokio::test]
async fn invalid_line_ranges_return_416_without_resource_bytes() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("lines.txt"), b"one\ntwo\nthree").unwrap();
    let app = make_test_router(dir.path(), sbx::AuthState::new());

    for range in [
        "lines=-1",
        "lines=4-5",
        "lines=2-1",
        "lines=1-2,lines=3-3",
        "bytes=0-1,lines=1-2",
        "lines=184467440737095516160-",
        "lines= 1-2",
    ] {
        let response = app
            .clone()
            .oneshot(request(Method::GET, "/lines.txt", Some(range)))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::RANGE_NOT_SATISFIABLE,
            "{range}"
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(!body.starts_with(b"one"), "{range} returned resource bytes");
    }
}

#[tokio::test]
async fn binary_does_not_advertise_or_serve_line_ranges() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("data.bin"), [0, 1, 2, 3]).unwrap();
    let app = make_test_router(dir.path(), sbx::AuthState::new());

    let response = app
        .clone()
        .oneshot(request(Method::GET, "/data.bin", None))
        .await
        .unwrap();
    assert_eq!(response.headers()["accept-ranges"], "bytes");

    let response = app
        .oneshot(request(Method::GET, "/data.bin", Some("lines=1-1")))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    assert!(response.headers().get("content-range").is_none());
    assert!(
        axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn head_ignores_range_and_if_range() {
    let dir = tempfile::TempDir::new().unwrap();
    let data = b"one\ntwo\nthree";
    std::fs::write(dir.path().join("lines.txt"), data).unwrap();
    let app = make_test_router(dir.path(), sbx::AuthState::new());
    let response = app
        .oneshot(request(Method::HEAD, "/lines.txt", Some("lines=2-2")))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-length"], data.len().to_string());
    assert!(response.headers().get("content-range").is_none());
    assert!(
        axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn if_range_controls_range_processing() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("lines.txt"), b"one\ntwo\nthree").unwrap();
    let app = make_test_router(dir.path(), sbx::AuthState::new());
    let full = app
        .clone()
        .oneshot(request(Method::GET, "/lines.txt", None))
        .await
        .unwrap();
    let validator = full.headers()["last-modified"].to_str().unwrap().to_owned();
    let stale = Request::builder()
        .method(Method::GET)
        .uri("/lines.txt")
        .header("range", "lines=2-2")
        .header("if-range", "stale-validator")
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(stale).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
        &b"one\ntwo\nthree"[..]
    );

    let matching = Request::builder()
        .method(Method::GET)
        .uri("/lines.txt")
        .header("range", "lines=2-2")
        .header("if-range", validator)
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(matching).await.unwrap();
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
}

#[tokio::test]
async fn line_range_preserves_not_found_and_authentication_behavior() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("lines.txt"), b"one\ntwo").unwrap();
    let mut auth = sbx::AuthState::new();
    auth.add_user("reader", "secret");
    let app = make_test_router(dir.path(), auth);

    let missing = app
        .clone()
        .oneshot(request(Method::GET, "/missing.txt", Some("lines=1-1")))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);

    let unauthorized = app
        .clone()
        .oneshot(request(Method::GET, "/lines.txt", Some("lines=1-1")))
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let authorized = Request::builder()
        .method(Method::GET)
        .uri("/missing.txt")
        .header("authorization", "Basic cmVhZGVyOnNlY3JldA==")
        .header("range", "lines=1-1")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(authorized).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
