mod common;

use axum::body::Body;
use axum::http::Method;
use common::{make_test_router, temp_dir_with_files};
use tower::ServiceExt;

#[tokio::test]
async fn test_get_root_dir_listing() {
    let dir = temp_dir_with_files();
    let app = make_test_router(dir.path(), rshs::AuthState::new());

    let req = axum::http::Request::builder()
        .method(Method::GET)
        .uri("/")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("Index of /"), "should contain title");
    assert!(html.contains("hello.txt"), "should list hello.txt");
    assert!(html.contains("subdir/"), "should list subdir");
    assert!(!html.contains("../"), "root should not have parent link");
}

#[tokio::test]
async fn test_get_file_content() {
    let dir = temp_dir_with_files();
    let app = make_test_router(dir.path(), rshs::AuthState::new());

    let req = axum::http::Request::builder()
        .method(Method::GET)
        .uri("/hello.txt")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert!(resp.status().is_success());

    let ct = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        ct.contains("text/plain"),
        "content-type should be text/plain"
    );

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(body.as_ref(), b"Hello, World!");
}

#[tokio::test]
async fn test_head_file() {
    let dir = temp_dir_with_files();
    let app = make_test_router(dir.path(), rshs::AuthState::new());

    let req = axum::http::Request::builder()
        .method(Method::HEAD)
        .uri("/hello.txt")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert!(resp.status().is_success());
    assert!(resp.headers().contains_key("content-length"));

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(body.is_empty(), "HEAD should have empty body");
}

#[tokio::test]
async fn test_get_not_found() {
    let dir = temp_dir_with_files();
    let app = make_test_router(dir.path(), rshs::AuthState::new());

    let req = axum::http::Request::builder()
        .method(Method::GET)
        .uri("/nonexistent.txt")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 404);
}

#[tokio::test]
async fn test_get_nested_file() {
    let dir = temp_dir_with_files();
    let app = make_test_router(dir.path(), rshs::AuthState::new());

    let req = axum::http::Request::builder()
        .method(Method::GET)
        .uri("/subdir/nested.txt")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert!(resp.status().is_success());

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(body.as_ref(), b"Nested file");
}

#[tokio::test]
async fn test_get_path_traversal_blocked() {
    let dir = temp_dir_with_files();
    let app = make_test_router(dir.path(), rshs::AuthState::new());

    let req = axum::http::Request::builder()
        .method(Method::GET)
        .uri("/../outside.txt")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 404);
}

#[tokio::test]
async fn test_put_creates_new_file() {
    let dir = temp_dir_with_files();
    let app = make_test_router(dir.path(), rshs::AuthState::new());

    let req = axum::http::Request::builder()
        .method(Method::PUT)
        .uri("/newfile.txt")
        .header("content-type", "text/plain")
        .body(Body::from("hello put"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 201);

    let content = std::fs::read_to_string(dir.path().join("newfile.txt")).unwrap();
    assert_eq!(content, "hello put");
}

#[tokio::test]
async fn test_put_overwrites_existing_file() {
    let dir = temp_dir_with_files();
    let app = make_test_router(dir.path(), rshs::AuthState::new());

    let req = axum::http::Request::builder()
        .method(Method::PUT)
        .uri("/hello.txt")
        .header("content-type", "text/plain")
        .body(Body::from("overwritten"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);

    let content = std::fs::read_to_string(dir.path().join("hello.txt")).unwrap();
    assert_eq!(content, "overwritten");
}

#[tokio::test]
async fn test_delete_existing_file() {
    let dir = temp_dir_with_files();
    let app = make_test_router(dir.path(), rshs::AuthState::new());

    let req = axum::http::Request::builder()
        .method(Method::DELETE)
        .uri("/hello.txt")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 204);
    assert!(!dir.path().join("hello.txt").exists());
}

#[tokio::test]
async fn test_delete_nonexistent() {
    let dir = temp_dir_with_files();
    let app = make_test_router(dir.path(), rshs::AuthState::new());

    let req = axum::http::Request::builder()
        .method(Method::DELETE)
        .uri("/nonexistent.txt")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 404);
}

#[tokio::test]
async fn test_patch_overwrites_requested_range() {
    let dir = temp_dir_with_files();
    let app = make_test_router(dir.path(), rshs::AuthState::new());

    let req = axum::http::Request::builder()
        .method(Method::PATCH)
        .uri("/hello.txt")
        .header("content-type", "application/x-sabredav-partialupdate")
        .header("x-update-range", "bytes=7-11")
        .header("content-length", "5")
        .body(Body::from("Rust!"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 204);
    assert_eq!(
        std::fs::read(dir.path().join("hello.txt")).unwrap(),
        b"Hello, Rust!!"
    );
}

#[tokio::test]
async fn test_patch_appends_to_file() {
    let dir = temp_dir_with_files();
    let app = make_test_router(dir.path(), rshs::AuthState::new());

    let req = axum::http::Request::builder()
        .method(Method::PATCH)
        .uri("/hello.txt")
        .header("content-type", "application/x-sabredav-partialupdate")
        .header("x-update-range", "append")
        .header("content-length", "9")
        .body(Body::from(" appended"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 204);
    assert_eq!(
        std::fs::read(dir.path().join("hello.txt")).unwrap(),
        b"Hello, World! appended"
    );
}

#[tokio::test]
async fn test_patch_supports_negative_suffix_range() {
    let dir = temp_dir_with_files();
    let app = make_test_router(dir.path(), rshs::AuthState::new());

    let req = axum::http::Request::builder()
        .method(Method::PATCH)
        .uri("/hello.txt")
        .header("content-type", "application/x-sabredav-partialupdate")
        .header("x-update-range", "bytes=-5")
        .header("content-length", "5")
        .body(Body::from("Rust!"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 204);
    assert_eq!(
        std::fs::read(dir.path().join("hello.txt")).unwrap(),
        b"Hello, WRust!"
    );
}

#[tokio::test]
async fn test_patch_fills_gap_with_zeroes() {
    let dir = temp_dir_with_files();
    let app = make_test_router(dir.path(), rshs::AuthState::new());

    let req = axum::http::Request::builder()
        .method(Method::PATCH)
        .uri("/hello.txt")
        .header("content-type", "application/x-sabredav-partialupdate")
        .header("x-update-range", "bytes=16-")
        .header("content-length", "3")
        .body(Body::from("end"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 204);
    assert_eq!(
        std::fs::read(dir.path().join("hello.txt")).unwrap(),
        b"Hello, World!\0\0\0end"
    );
}

#[tokio::test]
async fn test_patch_requires_content_length_and_range() {
    let dir = temp_dir_with_files();
    let app = make_test_router(dir.path(), rshs::AuthState::new());

    let req = axum::http::Request::builder()
        .method(Method::PATCH)
        .uri("/hello.txt")
        .header("content-type", "application/x-sabredav-partialupdate")
        .header("x-update-range", "append")
        .body(Body::from("data"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 411);

    let req = axum::http::Request::builder()
        .method(Method::PATCH)
        .uri("/hello.txt")
        .header("content-type", "application/x-sabredav-partialupdate")
        .header("content-length", "4")
        .body(Body::from("data"))
        .unwrap();
    let resp = make_test_router(dir.path(), rshs::AuthState::new())
        .oneshot(req)
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn test_patch_rejects_unsupported_content_type() {
    let dir = temp_dir_with_files();
    let app = make_test_router(dir.path(), rshs::AuthState::new());

    let req = axum::http::Request::builder()
        .method(Method::PATCH)
        .uri("/hello.txt")
        .header("content-type", "text/plain")
        .header("x-update-range", "append")
        .header("content-length", "3")
        .body(Body::from("bad"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 415);
}

#[tokio::test]
async fn test_patch_rejects_missing_content_type() {
    let dir = temp_dir_with_files();
    let app = make_test_router(dir.path(), rshs::AuthState::new());

    let req = axum::http::Request::builder()
        .method(Method::PATCH)
        .uri("/hello.txt")
        .header("x-update-range", "append")
        .header("content-length", "3")
        .body(Body::from("bad"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 415);
}

#[tokio::test]
async fn test_patch_rejects_body_length_mismatch() {
    let dir = temp_dir_with_files();

    let req = axum::http::Request::builder()
        .method(Method::PATCH)
        .uri("/hello.txt")
        .header("content-type", "application/x-sabredav-partialupdate")
        .header("x-update-range", "bytes=0-3")
        .header("content-length", "8")
        .body(Body::from("too long"))
        .unwrap();
    let resp = make_test_router(dir.path(), rshs::AuthState::new())
        .oneshot(req)
        .await
        .unwrap();
    assert_eq!(resp.status(), 416);
}

#[tokio::test]
async fn test_options_returns_allow_header() {
    let dir = temp_dir_with_files();
    let app = make_test_router(dir.path(), rshs::AuthState::new());

    let req = axum::http::Request::builder()
        .method(Method::OPTIONS)
        .uri("/")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);

    let allow = resp.headers().get("allow").unwrap().to_str().unwrap();
    assert!(allow.contains("GET"), "Allow should include GET");
    assert!(allow.contains("PUT"), "Allow should include PUT");
    assert!(allow.contains("PATCH"), "Allow should include PATCH");
    assert!(allow.contains("PROPFIND"), "Allow should include PROPFIND");

    let dav = resp.headers().get("dav").unwrap().to_str().unwrap();
    assert_eq!(dav, "1, 2", "DAV header should advertise DAV support");

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(body.is_empty(), "OPTIONS body should be empty");
}

#[tokio::test]
async fn test_unknown_method_returns_501() {
    let dir = temp_dir_with_files();
    let app = make_test_router(dir.path(), rshs::AuthState::new());

    let req = axum::http::Request::builder()
        .method(Method::POST)
        .uri("/")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 501);
}
