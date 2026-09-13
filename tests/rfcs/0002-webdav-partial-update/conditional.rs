use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use tower::ServiceExt;

use crate::common::make_test_router;

fn lock_body() -> Body {
    Body::from(
        r#"<?xml version="1.0"?><D:lockinfo xmlns:D="DAV:"><D:lockscope><D:exclusive/></D:lockscope><D:locktype><D:write/></D:locktype></D:lockinfo>"#,
    )
}

#[tokio::test]
async fn if_match_and_lock_tokens_are_enforced() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("file.bin"), b"1234").unwrap();
    let app = make_test_router(dir.path(), sbx::AuthState::new());

    let stale = Request::builder()
        .method(Method::PATCH)
        .uri("/file.bin")
        .header("content-type", "application/partial-update")
        .header("content-length", "1")
        .header("x-update-range", "bytes=0-0")
        .header("if-match", "\"stale\"")
        .body(Body::from("x"))
        .unwrap();
    let response = app.clone().oneshot(stale).await.unwrap();
    assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);
    assert_eq!(std::fs::read(dir.path().join("file.bin")).unwrap(), b"1234");

    let lock = Request::builder()
        .method(Method::from_bytes(b"LOCK").unwrap())
        .uri("/file.bin")
        .body(lock_body())
        .unwrap();
    let lock_response = app.clone().oneshot(lock).await.unwrap();
    assert_eq!(lock_response.status(), StatusCode::OK);
    let token = lock_response.headers()["lock-token"].clone();

    let blocked = Request::builder()
        .method(Method::PATCH)
        .uri("/file.bin")
        .header("content-type", "application/partial-update")
        .header("content-length", "1")
        .header("x-update-range", "bytes=0-0")
        .body(Body::from("x"))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(blocked).await.unwrap().status(),
        StatusCode::LOCKED
    );

    let mut allowed = Request::builder()
        .method(Method::PATCH)
        .uri("/file.bin")
        .header("content-type", "application/partial-update")
        .header("content-length", "1")
        .header("x-update-range", "bytes=0-0")
        .body(Body::from("x"))
        .unwrap();
    allowed.headers_mut().insert("lock-token", token.clone());
    assert_eq!(
        app.clone().oneshot(allowed).await.unwrap().status(),
        StatusCode::NO_CONTENT
    );

    let wrong_token = Request::builder()
        .method(Method::PATCH)
        .uri("/file.bin")
        .header("content-type", "application/partial-update")
        .header("content-length", "1")
        .header("x-update-range", "bytes=0-0")
        .header("lock-token", "<opaquelocktoken:wrong>")
        .body(Body::from("y"))
        .unwrap();
    assert_eq!(
        app.oneshot(wrong_token).await.unwrap().status(),
        StatusCode::LOCKED
    );
}
