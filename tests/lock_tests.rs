mod common;

use axum::body::Body;
use common::{make_test_router, temp_dir_with_files};
use tower::ServiceExt;

fn make_request(method: &str, uri: &str, body: Body) -> axum::http::Request<Body> {
    use std::str::FromStr;
    axum::http::Request::builder()
        .method(axum::http::Method::from_str(method).unwrap())
        .uri(uri)
        .body(body)
        .unwrap()
}

fn lock_body(exclusive: bool) -> Body {
    let scope = if exclusive { "exclusive" } else { "shared" };
    Body::from(format!(
        r#"<?xml version="1.0" encoding="utf-8"?><D:lockinfo xmlns:D="DAV:"><D:lockscope><D:{scope}/></D:lockscope><D:locktype><D:write/></D:locktype></D:lockinfo>"#
    ))
}

#[tokio::test]
async fn test_lock_existing_file() {
    let dir = temp_dir_with_files();
    let app = make_test_router(dir.path(), sbx::AuthState::new());

    let req = make_request("LOCK", "/hello.txt", lock_body(true));
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);

    let token = resp.headers().get("lock-token").unwrap().to_str().unwrap();
    assert!(token.contains("opaquelocktoken:"));

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let xml = String::from_utf8(body.to_vec()).unwrap();
    assert!(xml.contains("D:activelock"));
    assert!(xml.contains("D:exclusive"));
}

#[tokio::test]
async fn test_lock_nonexistent_creates_locknull() {
    let dir = temp_dir_with_files();
    let app = make_test_router(dir.path(), sbx::AuthState::new());

    let req = make_request("LOCK", "/nonexistent.txt", lock_body(true));
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 201);
    assert!(resp.headers().get("lock-token").is_some());
}

#[tokio::test]
async fn test_shared_lock_succeeds() {
    let dir = temp_dir_with_files();
    let app = make_test_router(dir.path(), sbx::AuthState::new());

    let req = make_request("LOCK", "/hello.txt", lock_body(false));
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let xml = String::from_utf8(body.to_vec()).unwrap();
    assert!(xml.contains("D:shared"));
}

#[tokio::test]
async fn test_double_shared_lock_succeeds() {
    let dir = temp_dir_with_files();
    // Two separate routers — separate state, so no conflict
    let app1 = make_test_router(dir.path(), sbx::AuthState::new());
    let app2 = make_test_router(dir.path(), sbx::AuthState::new());

    let req = make_request("LOCK", "/hello.txt", lock_body(false));
    let resp = app1.oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);

    let req = make_request("LOCK", "/hello.txt", lock_body(false));
    let resp = app2.oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
}

#[tokio::test]
async fn test_exclusive_lock_blocks_second_lock() {
    let dir = temp_dir_with_files();
    let app = make_test_router(dir.path(), sbx::AuthState::new());

    let req = make_request("LOCK", "/hello.txt", lock_body(true));
    let _resp = app.clone().oneshot(req).await.unwrap();

    let req = make_request("LOCK", "/hello.txt", lock_body(true));
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 423);
}

#[tokio::test]
async fn test_unlock_with_correct_token() {
    let dir = temp_dir_with_files();
    let app = make_test_router(dir.path(), sbx::AuthState::new());

    // Lock
    let req = make_request("LOCK", "/hello.txt", lock_body(true));
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let token = resp
        .headers()
        .get("lock-token")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    // Unlock with the same token
    let mut req = make_request("UNLOCK", "/hello.txt", Body::empty());
    req.headers_mut()
        .insert("lock-token", token.parse().unwrap());
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 204);
}

#[tokio::test]
async fn test_unlock_with_wrong_token_returns_403() {
    let dir = temp_dir_with_files();
    let app = make_test_router(dir.path(), sbx::AuthState::new());

    let req = make_request("LOCK", "/hello.txt", lock_body(true));
    let _resp = app.clone().oneshot(req).await.unwrap();

    let mut req = make_request("UNLOCK", "/hello.txt", Body::empty());
    req.headers_mut()
        .insert("lock-token", "<opaquelocktoken:wrong>".parse().unwrap());
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 403);
}

#[tokio::test]
async fn test_put_on_locked_resource_without_token_returns_423() {
    let dir = temp_dir_with_files();
    let app = make_test_router(dir.path(), sbx::AuthState::new());

    let req = make_request("LOCK", "/hello.txt", lock_body(true));
    let _resp = app.clone().oneshot(req).await.unwrap();

    let req = axum::http::Request::builder()
        .method(axum::http::Method::PUT)
        .uri("/hello.txt")
        .body(Body::from("blocked"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 423);
}

#[tokio::test]
async fn test_delete_on_locked_resource_returns_423() {
    let dir = temp_dir_with_files();
    let app = make_test_router(dir.path(), sbx::AuthState::new());

    let req = make_request("LOCK", "/hello.txt", lock_body(true));
    let _resp = app.clone().oneshot(req).await.unwrap();

    let req = axum::http::Request::builder()
        .method(axum::http::Method::DELETE)
        .uri("/hello.txt")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 423);
}
