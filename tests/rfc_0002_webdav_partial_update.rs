mod common;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use tower::ServiceExt;

use crate::common::make_test_router;

mod conditional {
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
}

fn patch(
    path: &str, content_type: Option<&str>, range: Option<&str>, body: &[u8],
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(Method::PATCH)
        .uri(path)
        .header("content-length", body.len());
    if let Some(value) = content_type {
        builder = builder.header("content-type", value);
    }
    if let Some(value) = range {
        builder = builder.header("x-update-range", value);
    }
    builder.body(Body::from(body.to_vec())).unwrap()
}

#[tokio::test]
async fn options_advertises_partial_update_capability() {
    let dir = tempfile::TempDir::new().unwrap();
    let app = make_test_router(dir.path(), sbx::AuthState::new());
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::OPTIONS)
                .uri("/")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers()["allow"]
            .to_str()
            .unwrap()
            .contains("PATCH")
    );
    assert!(
        response.headers()["dav"]
            .to_str()
            .unwrap()
            .contains("partial-update")
    );
}

#[tokio::test]
async fn interval_append_open_ended_and_suffix_updates() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("file.bin"), b"1234567890").unwrap();
    let app = make_test_router(dir.path(), sbx::AuthState::new());

    let response = app
        .clone()
        .oneshot(patch(
            "/file.bin",
            Some("application/partial-update; version=1"),
            Some("bytes=3-6"),
            b"----",
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(
        axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        std::fs::read(dir.path().join("file.bin")).unwrap(),
        b"123----890"
    );

    let response = app
        .clone()
        .oneshot(patch(
            "/file.bin",
            Some("application/partial-update"),
            Some("bytes=3-"),
            b"++++",
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        std::fs::read(dir.path().join("file.bin")).unwrap(),
        b"123++++890"
    );

    let response = app
        .clone()
        .oneshot(patch(
            "/file.bin",
            Some("application/partial-update"),
            Some("bytes=-4"),
            b"TAIL",
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        std::fs::read(dir.path().join("file.bin")).unwrap(),
        b"123+++TAIL"
    );

    let response = app
        .oneshot(patch(
            "/file.bin",
            Some("application/partial-update"),
            Some("append"),
            b"END",
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        std::fs::read(dir.path().join("file.bin")).unwrap(),
        b"123+++TAILEND"
    );
}

#[tokio::test]
async fn out_of_bounds_updates_grow_and_zero_fill() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("file.bin"), b"1234567890").unwrap();
    let app = make_test_router(dir.path(), sbx::AuthState::new());
    let response = app
        .oneshot(patch(
            "/file.bin",
            Some("application/partial-update"),
            Some("bytes=12-"),
            b"----",
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        std::fs::read(dir.path().join("file.bin")).unwrap(),
        b"1234567890\0\0----"
    );
}

#[tokio::test]
async fn empty_append_and_binary_payload_are_supported() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("file.bin"), [0, 255, 1]).unwrap();
    let app = make_test_router(dir.path(), sbx::AuthState::new());
    let response = app
        .clone()
        .oneshot(patch(
            "/file.bin",
            Some("application/partial-update"),
            Some("append"),
            &[],
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        std::fs::read(dir.path().join("file.bin")).unwrap(),
        [0, 255, 1]
    );

    let response = app
        .oneshot(patch(
            "/file.bin",
            Some("application/partial-update"),
            Some("bytes=1-2"),
            &[0, 0],
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        std::fs::read(dir.path().join("file.bin")).unwrap(),
        [0, 0, 0]
    );
}

#[tokio::test]
async fn malformed_and_inconsistent_ranges_are_rejected_without_writes() {
    let dir = tempfile::TempDir::new().unwrap();
    let original = b"1234567890";
    std::fs::write(dir.path().join("file.bin"), original).unwrap();
    let app = make_test_router(dir.path(), sbx::AuthState::new());
    for range in [
        "bytes=2-8",
        "bytes=8-2",
        "bytes=-0",
        "bytes=-",
        "bytes=1-2,bytes=3-4",
        "bytes=184467440737095516160-",
        "bytes= 1-2",
        "ap pend",
        "unknown",
    ] {
        let response = app
            .clone()
            .oneshot(patch(
                "/file.bin",
                Some("application/partial-update"),
                Some(range),
                b"----",
            ))
            .await
            .unwrap();
        assert!(
            matches!(
                response.status(),
                StatusCode::BAD_REQUEST | StatusCode::RANGE_NOT_SATISFIABLE
            ),
            "{range}: {}",
            response.status()
        );
        assert_eq!(
            std::fs::read(dir.path().join("file.bin")).unwrap(),
            original,
            "{range} modified resource"
        );
    }
    let response = app
        .oneshot(patch(
            "/file.bin",
            Some("application/partial-update"),
            Some("bytes=1-2"),
            b"x",
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(
        std::fs::read(dir.path().join("file.bin")).unwrap(),
        original
    );
}

#[tokio::test]
async fn missing_headers_and_media_type_fail_with_specified_statuses() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("file.bin"), b"1234").unwrap();
    let app = make_test_router(dir.path(), sbx::AuthState::new());

    let missing_type = Request::builder()
        .method(Method::PATCH)
        .uri("/file.bin")
        .header("content-length", 1)
        .header("x-update-range", "append")
        .body(Body::from("x"))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(missing_type).await.unwrap().status(),
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
    let missing_range = Request::builder()
        .method(Method::PATCH)
        .uri("/file.bin")
        .header("content-type", "application/partial-update")
        .header("content-length", 1)
        .body(Body::from("x"))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(missing_range).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
    let missing_length = Request::builder()
        .method(Method::PATCH)
        .uri("/file.bin")
        .header("content-type", "application/partial-update")
        .header("x-update-range", "append")
        .body(Body::from("x"))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(missing_length).await.unwrap().status(),
        StatusCode::LENGTH_REQUIRED
    );
    let wrong_type = app
        .oneshot(patch(
            "/file.bin",
            Some("application/octet-stream"),
            Some("append"),
            b"x",
        ))
        .await
        .unwrap();
    assert_eq!(wrong_type.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn not_found_traversal_and_auth_are_preserved() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("file.bin"), b"1234").unwrap();
    let mut auth = sbx::AuthState::new();
    auth.add_user("writer", "secret");
    let app = make_test_router(dir.path(), auth);
    let unauthorized = app
        .clone()
        .oneshot(patch(
            "/file.bin",
            Some("application/partial-update"),
            Some("append"),
            b"x",
        ))
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    let missing = Request::builder()
        .method(Method::PATCH)
        .uri("/missing.bin")
        .header("authorization", "Basic d3JpdGVyOnNlY3JldA==")
        .header("content-type", "application/partial-update")
        .header("content-length", 1)
        .header("x-update-range", "append")
        .body(Body::from("x"))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(missing).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
    let traversal = Request::builder()
        .method(Method::PATCH)
        .uri("/../outside.bin")
        .header("content-type", "application/partial-update")
        .header("content-length", 1)
        .header("x-update-range", "append")
        .body(Body::from("x"))
        .unwrap();
    assert_eq!(
        app.oneshot(traversal).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn successful_update_changes_validator_and_serializes_writes() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("file.bin"), b"1234").unwrap();
    let app = make_test_router(dir.path(), sbx::AuthState::new());
    let before = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/file.bin")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let old_etag = before.headers().get("etag").cloned();
    let response = app
        .clone()
        .oneshot(patch(
            "/file.bin",
            Some("application/partial-update"),
            Some("append"),
            b"x",
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let after = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/file.bin")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(after.headers().get("etag"), old_etag.as_ref());
}
