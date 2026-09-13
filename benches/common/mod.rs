#![allow(dead_code)]

use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use axum::Router;
use axum::body::{self, Body};
use axum::extract::Request;
use axum::http::Method;
use sbx::{AppState, AuthState, make_router};

static COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn bench_router(root_dir: &Path) -> Router {
    let state = Arc::new(AppState::new(
        root_dir.to_path_buf(),
        AuthState::new(),
        Duration::from_secs(300),
    ));
    make_router(state)
}

pub fn bench_router_with_auth(root_dir: &Path, auth: AuthState) -> Router {
    let state = Arc::new(AppState::new(
        root_dir.to_path_buf(),
        auth,
        Duration::from_secs(300),
    ));
    make_router(state)
}

pub fn unique_name(prefix: &str) -> String {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}_{n}")
}

pub fn create_file(dir: &Path, name: &str, size: usize) {
    let path = dir.join(name);
    let mut f = std::fs::File::create(&path).unwrap();
    let content = vec![b'a'; size];
    f.write_all(&content).unwrap();
}

pub fn create_dir(dir: &Path, name: &str) {
    std::fs::create_dir(dir.join(name)).unwrap();
}

pub fn create_files(dir: &Path, count: usize, size: usize) {
    for i in 0..count {
        create_file(dir, &format!("file_{i:05}.bin"), size);
    }
}

pub fn create_small_files(dir: &Path, count: usize) {
    for i in 0..count {
        let path = dir.join(format!("file_{i:05}.txt"));
        let mut f = std::fs::File::create(&path).unwrap();
        write!(f, "content_{i}").unwrap();
    }
}

pub fn create_nested_tree(dir: &Path, depth: usize, breadth: usize) {
    let mut current = dir.to_path_buf();
    for d in 0..depth {
        let dir_name = format!("level_{d}");
        let next = current.join(&dir_name);
        std::fs::create_dir(&next).unwrap();
        for b in 0..breadth {
            create_file(&next, &format!("file_{b}.txt"), 1024);
        }
        // Add a subdir leaf at each level
        let sub = next.join("subdir");
        std::fs::create_dir(&sub).unwrap();
        create_file(&sub, "leaf.txt", 512);
        current = next;
    }
}

pub fn make_get(uri: &str) -> Request {
    Request::builder()
        .method(Method::GET)
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

pub fn make_head(uri: &str) -> Request {
    Request::builder()
        .method(Method::HEAD)
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

pub fn make_put(uri: &str, body: &[u8]) -> Request {
    Request::builder()
        .method(Method::PUT)
        .uri(uri)
        .body(Body::from(body.to_vec()))
        .unwrap()
}

pub fn make_delete(uri: &str) -> Request {
    Request::builder()
        .method(Method::DELETE)
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

pub fn make_propfind(uri: &str, depth: &str, body: &[u8]) -> Request {
    Request::builder()
        .method(Method::from_bytes(b"PROPFIND").unwrap())
        .uri(uri)
        .header("depth", depth)
        .body(Body::from(body.to_vec()))
        .unwrap()
}

pub fn make_mkcol(uri: &str) -> Request {
    Request::builder()
        .method(Method::from_bytes(b"MKCOL").unwrap())
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

pub fn make_copy(uri: &str, destination: &str) -> Request {
    Request::builder()
        .method(Method::from_bytes(b"COPY").unwrap())
        .uri(uri)
        .header("destination", destination)
        .body(Body::empty())
        .unwrap()
}

pub fn make_move(uri: &str, destination: &str) -> Request {
    Request::builder()
        .method(Method::from_bytes(b"MOVE").unwrap())
        .uri(uri)
        .header("destination", destination)
        .body(Body::empty())
        .unwrap()
}

pub fn make_lock(uri: &str, body: &[u8]) -> Request {
    Request::builder()
        .method(Method::from_bytes(b"LOCK").unwrap())
        .uri(uri)
        .body(Body::from(body.to_vec()))
        .unwrap()
}

pub fn make_unlock(uri: &str, token: &str) -> Request {
    Request::builder()
        .method(Method::from_bytes(b"UNLOCK").unwrap())
        .uri(uri)
        .header("lock-token", format!("<{token}>"))
        .body(Body::empty())
        .unwrap()
}

pub fn make_options(uri: &str) -> Request {
    Request::builder()
        .method(Method::OPTIONS)
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

pub fn make_health_check(uri: &str) -> Request {
    Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header("x-health-check", "true")
        .body(Body::empty())
        .unwrap()
}

pub fn make_basic_auth_get(uri: &str, username: &str, password: &str) -> Request {
    use base64::Engine;
    let creds = base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}"));
    Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header("authorization", format!("Basic {creds}"))
        .body(Body::empty())
        .unwrap()
}

pub fn lock_body_exclusive() -> Vec<u8> {
    br#"<?xml version="1.0"?><D:lockinfo xmlns:D="DAV:"><D:lockscope><D:exclusive/></D:lockscope><D:locktype><D:write/></D:locktype></D:lockinfo>"#.to_vec()
}

pub fn lock_body_shared() -> Vec<u8> {
    br#"<?xml version="1.0"?><D:lockinfo xmlns:D="DAV:"><D:lockscope><D:shared/></D:lockscope><D:locktype><D:write/></D:locktype></D:lockinfo>"#.to_vec()
}

pub async fn drain_body(resp: axum::response::Response) {
    let _ = body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
}
