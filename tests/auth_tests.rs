mod common;

use axum::body::Body;
use axum::http::Method;
use base64::{Engine as _, engine::general_purpose};
use common::{make_test_router, temp_dir_with_files};
use tower::ServiceExt;

fn basic_auth_header(username: &str, password: &str) -> String {
    let creds = format!("{username}:{password}");
    format!(
        "Basic {}",
        general_purpose::STANDARD.encode(creds.as_bytes())
    )
}

#[tokio::test]
async fn test_no_auth_passes_through() {
    let dir = temp_dir_with_files();
    let app = make_test_router(dir.path(), sbx::AuthState::new());

    let req = axum::http::Request::builder()
        .method(Method::GET)
        .uri("/hello.txt")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert!(resp.status().is_success(), "no auth should pass through");
}

#[tokio::test]
async fn test_auth_returns_401_without_credentials() {
    let dir = temp_dir_with_files();
    let mut auth = sbx::AuthState::new();
    auth.add_user("admin", "secret");
    let app = make_test_router(dir.path(), auth);

    let req = axum::http::Request::builder()
        .method(Method::GET)
        .uri("/")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 401);
    assert!(
        resp.headers()
            .get("www-authenticate")
            .unwrap()
            .to_str()
            .unwrap()
            .contains(r#"Basic realm="sbx""#)
    );
}

#[tokio::test]
async fn test_auth_success_with_valid_credentials() {
    let dir = temp_dir_with_files();
    let mut auth = sbx::AuthState::new();
    auth.add_user("admin", "secret");
    let app = make_test_router(dir.path(), auth);

    let req = axum::http::Request::builder()
        .method(Method::GET)
        .uri("/hello.txt")
        .header("authorization", basic_auth_header("admin", "secret"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(body.as_ref(), b"Hello, World!");
}

#[tokio::test]
async fn test_auth_wrong_password_returns_401() {
    let dir = temp_dir_with_files();
    let mut auth = sbx::AuthState::new();
    auth.add_user("admin", "secret");
    let app = make_test_router(dir.path(), auth);

    let req = axum::http::Request::builder()
        .method(Method::GET)
        .uri("/")
        .header("authorization", basic_auth_header("admin", "wrong"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 401);
}

#[tokio::test]
async fn test_auth_unknown_user_returns_401() {
    let dir = temp_dir_with_files();
    let mut auth = sbx::AuthState::new();
    auth.add_user("admin", "secret");
    let app = make_test_router(dir.path(), auth);

    let req = axum::http::Request::builder()
        .method(Method::GET)
        .uri("/")
        .header("authorization", basic_auth_header("nobody", "pass"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 401);
}

#[tokio::test]
async fn test_health_check_bypasses_auth() {
    let dir = temp_dir_with_files();
    let mut auth = sbx::AuthState::new();
    auth.add_user("admin", "secret");
    let app = make_test_router(dir.path(), auth);

    let req = axum::http::Request::builder()
        .method(Method::GET)
        .uri("/")
        .header("x-health-check", "true")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(body.as_ref(), b"OK");
}

#[tokio::test]
async fn test_health_check_with_wrong_value_passes_through() {
    let dir = temp_dir_with_files();
    let mut auth = sbx::AuthState::new();
    auth.add_user("admin", "secret");
    let app = make_test_router(dir.path(), auth);

    let req = axum::http::Request::builder()
        .method(Method::GET)
        .uri("/")
        .header("x-health-check", "false")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 401, "should be blocked by auth");
}

#[tokio::test]
async fn test_auth_sha512_cached_request() {
    use sha_crypt::PasswordHasher;

    let dir = temp_dir_with_files();
    let hash = sha_crypt::ShaCrypt::default()
        .hash_password("mypassword".as_bytes())
        .unwrap()
        .to_string();
    let mut auth = sbx::AuthState::new();
    auth.users
        .insert("admin".into(), sbx::auth::Credential::Sha512Crypt(hash));
    let app = make_test_router(dir.path(), auth);

    for _ in 0..3 {
        let req = axum::http::Request::builder()
            .method(Method::GET)
            .uri("/hello.txt")
            .header("authorization", basic_auth_header("admin", "mypassword"))
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status().as_u16(), 200);
    }
}
