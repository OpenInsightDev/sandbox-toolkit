mod common;

use axum::body::Body;
use axum::http::Method;
use common::{make_test_router, temp_dir_with_files};
use tower::ServiceExt;

#[tokio::test]
async fn test_get_root_dir_listing() {
    let dir = temp_dir_with_files();
    let app = make_test_router(dir.path(), sbx::AuthState::new());

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
    let app = make_test_router(dir.path(), sbx::AuthState::new());

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
    let app = make_test_router(dir.path(), sbx::AuthState::new());

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
    let app = make_test_router(dir.path(), sbx::AuthState::new());

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
    let app = make_test_router(dir.path(), sbx::AuthState::new());

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
    let app = make_test_router(dir.path(), sbx::AuthState::new());

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
    let app = make_test_router(dir.path(), sbx::AuthState::new());

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
    let app = make_test_router(dir.path(), sbx::AuthState::new());

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
    let app = make_test_router(dir.path(), sbx::AuthState::new());

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
    let app = make_test_router(dir.path(), sbx::AuthState::new());

    let req = axum::http::Request::builder()
        .method(Method::DELETE)
        .uri("/nonexistent.txt")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 404);
}

#[tokio::test]
async fn test_options_returns_allow_header() {
    let dir = temp_dir_with_files();
    let app = make_test_router(dir.path(), sbx::AuthState::new());

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
    assert!(allow.contains("PROPFIND"), "Allow should include PROPFIND");

    let dav = resp.headers().get("dav").unwrap().to_str().unwrap();
    assert_eq!(
        dav, "1, 2, 3, partial-update",
        "DAV header should advertise partial updates"
    );

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(body.is_empty(), "OPTIONS body should be empty");
}

#[tokio::test]
async fn test_patch_replaces_append_and_suffix_bytes() {
    let dir = temp_dir_with_files();
    std::fs::write(dir.path().join("patch.bin"), b"1234567890").unwrap();
    let app = make_test_router(dir.path(), sbx::AuthState::new());

    for (range, body, expected) in [
        ("bytes=3-6", b"----".as_slice(), b"123----890".as_slice()),
        ("append", b"!!".as_slice(), b"123----890!!".as_slice()),
        ("bytes=-2", b"??".as_slice(), b"123----890??".as_slice()),
    ] {
        let req = axum::http::Request::builder()
            .method(Method::PATCH)
            .uri("/patch.bin")
            .header("content-type", "application/partial-update; profile=test")
            .header("content-length", body.len())
            .header("x-update-range", range)
            .body(Body::from(body.to_vec()))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 204);
        assert!(
            axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            std::fs::read(dir.path().join("patch.bin")).unwrap(),
            expected
        );
    }
}

#[tokio::test]
async fn test_patch_fills_sparse_gap_and_rejects_bad_length_without_mutation() {
    let dir = temp_dir_with_files();
    std::fs::write(dir.path().join("patch.bin"), b"abc").unwrap();
    let app = make_test_router(dir.path(), sbx::AuthState::new());

    let req = axum::http::Request::builder()
        .method(Method::PATCH)
        .uri("/patch.bin")
        .header("content-type", "application/partial-update")
        .header("content-length", "2")
        .header("x-update-range", "bytes=5-")
        .body(Body::from("XY"))
        .unwrap();
    assert_eq!(app.clone().oneshot(req).await.unwrap().status(), 204);
    assert_eq!(
        std::fs::read(dir.path().join("patch.bin")).unwrap(),
        b"abc\0\0XY"
    );

    let before = std::fs::read(dir.path().join("patch.bin")).unwrap();
    let req = axum::http::Request::builder()
        .method(Method::PATCH)
        .uri("/patch.bin")
        .header("content-type", "application/partial-update")
        .header("content-length", "1")
        .header("x-update-range", "bytes=0-2")
        .body(Body::from("Z"))
        .unwrap();
    assert_eq!(app.oneshot(req).await.unwrap().status(), 416);
    assert_eq!(std::fs::read(dir.path().join("patch.bin")).unwrap(), before);
}

#[tokio::test]
async fn test_unknown_method_returns_501() {
    let dir = temp_dir_with_files();
    let app = make_test_router(dir.path(), sbx::AuthState::new());

    let req = axum::http::Request::builder()
        .method(Method::POST)
        .uri("/")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 501);
}
