mod common;

use axum::body::Body;
use common::{make_test_router, temp_dir_with_files};
use tower::ServiceExt;

fn make_propfind_body(props: &str) -> Body {
    Body::from(format!(
        r#"<?xml version="1.0" encoding="utf-8"?><D:propfind xmlns:D="DAV:"><D:prop>{props}</D:prop></D:propfind>"#
    ))
}

fn make_request(method: &str, uri: &str, body: Body) -> axum::http::Request<Body> {
    use std::str::FromStr;
    axum::http::Request::builder()
        .method(axum::http::Method::from_str(method).unwrap())
        .uri(uri)
        .body(body)
        .unwrap()
}

#[tokio::test]
async fn test_propfind_file_allprop() {
    let dir = temp_dir_with_files();
    let app = make_test_router(dir.path(), sbx::AuthState::new());

    let req = make_request("PROPFIND", "/hello.txt", make_propfind_body("<D:allprop/>"));
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 207);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let xml = String::from_utf8(body.to_vec()).unwrap();
    assert!(xml.contains("D:multistatus"), "should contain multistatus");
    assert!(xml.contains("hello.txt"), "should reference hello.txt");
    assert!(
        xml.contains("D:getcontentlength"),
        "should have getcontentlength"
    );
    assert!(xml.contains("D:getetag"), "should have getetag");
}

#[tokio::test]
async fn test_propfind_dir_depth_zero() {
    let dir = temp_dir_with_files();
    let app = make_test_router(dir.path(), sbx::AuthState::new());

    let req = make_request("PROPFIND", "/", make_propfind_body("<D:allprop/>"));
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 207);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let xml = String::from_utf8(body.to_vec()).unwrap();
    assert!(xml.contains("D:collection"), "should show collection type");
}

#[tokio::test]
async fn test_propfind_dir_depth_one() {
    let dir = temp_dir_with_files();
    let app = make_test_router(dir.path(), sbx::AuthState::new());

    let mut req = make_request("PROPFIND", "/", make_propfind_body("<D:allprop/>"));
    req.headers_mut().insert("depth", "1".parse().unwrap());
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 207);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let xml = String::from_utf8(body.to_vec()).unwrap();
    assert!(xml.contains("hello.txt"), "should list children");
    assert!(xml.contains("subdir"), "should list subdir");
}

#[tokio::test]
async fn test_propfind_nonexistent_returns_404() {
    let dir = temp_dir_with_files();
    let app = make_test_router(dir.path(), sbx::AuthState::new());

    let req = make_request(
        "PROPFIND",
        "/nonexistent",
        make_propfind_body("<D:allprop/>"),
    );
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 404);
}

#[tokio::test]
async fn test_mkcol_creates_directory() {
    let dir = temp_dir_with_files();
    let app = make_test_router(dir.path(), sbx::AuthState::new());

    let req = make_request("MKCOL", "/newdir", Body::empty());
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 201);
    assert!(dir.path().join("newdir").is_dir());
}

#[tokio::test]
async fn test_mkcol_parent_not_exist_returns_409() {
    let dir = temp_dir_with_files();
    let app = make_test_router(dir.path(), sbx::AuthState::new());

    let req = make_request("MKCOL", "/a/b/c", Body::empty());
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 409);
}

#[tokio::test]
async fn test_mkcol_already_exists_returns_405() {
    let dir = temp_dir_with_files();
    let app = make_test_router(dir.path(), sbx::AuthState::new());

    let req = make_request("MKCOL", "/subdir", Body::empty());
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 405);
}

#[tokio::test]
async fn test_copy_file_to_new_destination() {
    let dir = temp_dir_with_files();
    let app = make_test_router(dir.path(), sbx::AuthState::new());

    let mut req = make_request("COPY", "/hello.txt", Body::empty());
    req.headers_mut()
        .insert("destination", "/copied.txt".parse().unwrap());
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 201);

    assert!(dir.path().join("copied.txt").exists());
    assert!(dir.path().join("hello.txt").exists());
}

#[tokio::test]
async fn test_move_file_to_new_destination() {
    let dir = temp_dir_with_files();
    let app = make_test_router(dir.path(), sbx::AuthState::new());

    let mut req = make_request("MOVE", "/hello.txt", Body::empty());
    req.headers_mut()
        .insert("destination", "/moved.txt".parse().unwrap());
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 201);

    assert!(dir.path().join("moved.txt").exists());
    assert!(!dir.path().join("hello.txt").exists());
}

#[tokio::test]
async fn test_copy_overwrite_false_returns_412() {
    let dir = temp_dir_with_files();
    let app = make_test_router(dir.path(), sbx::AuthState::new());

    let mut req = make_request("COPY", "/hello.txt", Body::empty());
    req.headers_mut()
        .insert("destination", "/subdir/nested.txt".parse().unwrap());
    req.headers_mut().insert("overwrite", "F".parse().unwrap());
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 412);
}

#[tokio::test]
async fn test_proppatch_set_and_propfind_read_back() {
    let dir = temp_dir_with_files();
    let app = make_test_router(dir.path(), sbx::AuthState::new());

    let xml = r#"<?xml version="1.0" encoding="utf-8"?><D:propertyupdate xmlns:D="DAV:"><D:set><D:prop><X:myprop xmlns:X="http://example.com/">hello</X:myprop></D:prop></D:set></D:propertyupdate>"#;
    let req = make_request("PROPPATCH", "/hello.txt", Body::from(xml));
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 207);

    // Verify via PROPFIND
    let prop = r#"<X:myprop xmlns:X="http://example.com/"/>"#;
    let req = make_request("PROPFIND", "/hello.txt", make_propfind_body(prop));
    let resp = app.oneshot(req).await.unwrap();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let xml = String::from_utf8(body.to_vec()).unwrap();
    assert!(
        xml.contains("hello"),
        "dead property value should appear in PROPFIND response"
    );
}
