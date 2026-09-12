mod common;

use std::sync::Arc;
use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use tempfile::TempDir;
use tower::ServiceExt;

use axum::extract::Request;

use common::*;
use sbx::{AppState, AuthState, make_router};
use sha_crypt::PasswordHasher;

fn bench_health_check(c: &mut Criterion) {
    let dir = TempDir::new().unwrap();
    create_file(dir.path(), "hello.txt", 13);
    let router = bench_router(dir.path());
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("middleware/health_check");

    group.bench_function("intercept", |b| {
        b.iter(|| {
            rt.block_on(async {
                let _ = router
                    .clone()
                    .oneshot(make_health_check("/hello.txt"))
                    .await;
            });
        });
    });

    group.bench_function("passthrough_get", |b| {
        b.iter(|| {
            rt.block_on(async {
                let _ = router.clone().oneshot(make_get("/hello.txt")).await;
            });
        });
    });

    group.finish();
}

fn bench_auth(c: &mut Criterion) {
    let dir = TempDir::new().unwrap();
    create_file(dir.path(), "hello.txt", 13);
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("middleware/auth");

    // No users — auth middleware is a no-op
    let router_no_auth = bench_router(dir.path());
    group.bench_function("no_users_noop", |b| {
        b.iter(|| {
            rt.block_on(async {
                let _ = router_no_auth.clone().oneshot(make_get("/hello.txt")).await;
            });
        });
    });

    // With plaintext auth — valid credentials
    let mut auth = AuthState::new();
    auth.add_user("admin", "secret");
    let router_plain = bench_router_with_auth(dir.path(), auth);

    group.bench_function("plaintext_valid", |b| {
        b.iter(|| {
            let req = make_basic_auth_get("/hello.txt", "admin", "secret");
            rt.block_on(async {
                let _ = router_plain.clone().oneshot(req).await;
            });
        });
    });

    group.bench_function("plaintext_invalid", |b| {
        b.iter(|| {
            let req = make_basic_auth_get("/hello.txt", "admin", "wrong");
            rt.block_on(async {
                let _ = router_plain.clone().oneshot(req).await;
            });
        });
    });

    // With SHA-512 crypt auth
    let hash = sha_crypt::ShaCrypt::default()
        .hash_password("mypassword".as_bytes())
        .unwrap()
        .to_string();
    let mut sha_auth = AuthState::new();
    sha_auth
        .users
        .insert("admin".into(), sbx::auth::Credential::Sha512Crypt(hash));
    let router_sha = bench_router_with_auth(dir.path(), sha_auth);

    group.bench_function("sha512_valid", |b| {
        b.iter(|| {
            let req = make_basic_auth_get("/hello.txt", "admin", "mypassword");
            rt.block_on(async {
                let _ = router_sha.clone().oneshot(req).await;
            });
        });
    });

    group.bench_function("sha512_invalid", |b| {
        b.iter(|| {
            let req = make_basic_auth_get("/hello.txt", "admin", "wrong");
            rt.block_on(async {
                let _ = router_sha.clone().oneshot(req).await;
            });
        });
    });

    // Same credentials as sha512_valid — cache is warmed by criterion warmup cycle,
    // so measured iterations hit the cache and skip SHA-512 verification.
    group.bench_function("sha512_cached_hit", |b| {
        b.iter(|| {
            let req = make_basic_auth_get("/hello.txt", "admin", "mypassword");
            rt.block_on(async {
                let _ = router_sha.clone().oneshot(req).await;
            });
        });
    });

    group.finish();
}

fn bench_lock_enforce(c: &mut Criterion) {
    let body_1kb = vec![b'x'; 1024];
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("middleware/lock_enforce");

    // PUT on unlocked resource — lock middleware passes through
    let dir = TempDir::new().unwrap();
    create_file(dir.path(), "target.txt", 1024);
    let router_unlocked = bench_router(dir.path());

    group.bench_function("put_unlocked_passthrough", |b| {
        b.iter(|| {
            rt.block_on(async {
                let _ = router_unlocked
                    .clone()
                    .oneshot(make_put("/target.txt", &body_1kb))
                    .await;
            });
        });
    });

    // PUT on locked resource without token → 423
    group.bench_function("put_locked_rejected", |b| {
        b.iter(|| {
            let dir = TempDir::new().unwrap();
            create_file(dir.path(), "target.txt", 1024);
            let state = Arc::new(AppState::new(
                dir.path().to_path_buf(),
                AuthState::new(),
                Duration::from_secs(300),
            ));
            let lock = sbx::webdav::LockInfo::new(
                sbx::webdav::LockScope::Exclusive,
                "opaquelocktoken:t1".into(),
                None,
                std::time::SystemTime::now(),
                None,
                sbx::webdav::Depth::Zero,
            );
            rt.block_on(async {
                let mut locks = state.locks.write().await;
                locks.insert(dir.path().join("target.txt"), vec![lock]);
            });
            let router = make_router(state);
            rt.block_on(async {
                let _ = router.oneshot(make_put("/target.txt", &body_1kb)).await;
            });
        });
    });

    // PUT on locked resource with matching If token
    group.bench_function("put_locked_with_token", |b| {
        b.iter(|| {
            let dir = TempDir::new().unwrap();
            create_file(dir.path(), "target.txt", 1024);
            let state = Arc::new(AppState::new(
                dir.path().to_path_buf(),
                AuthState::new(),
                Duration::from_secs(300),
            ));
            let lock = sbx::webdav::LockInfo::new(
                sbx::webdav::LockScope::Exclusive,
                "opaquelocktoken:t1".into(),
                None,
                std::time::SystemTime::now(),
                None,
                sbx::webdav::Depth::Zero,
            );
            rt.block_on(async {
                let mut locks = state.locks.write().await;
                locks.insert(dir.path().join("target.txt"), vec![lock]);
            });
            let router = make_router(state);
            rt.block_on(async {
                let req = Request::builder()
                    .method(axum::http::Method::PUT)
                    .uri("/target.txt")
                    .header("if", "(<opaquelocktoken:t1>)")
                    .body(axum::body::Body::from(body_1kb.clone()))
                    .unwrap();
                let _ = router.oneshot(req).await;
            });
        });
    });

    // PUT on resource with ancestor lock (depth:infinity)
    group.bench_function("put_ancestor_locked_rejected", |b| {
        b.iter(|| {
            let dir = TempDir::new().unwrap();
            create_dir(dir.path(), "locked_parent");
            create_file(dir.path(), "locked_parent/deep.txt", 1024);
            let state = Arc::new(AppState::new(
                dir.path().to_path_buf(),
                AuthState::new(),
                Duration::from_secs(300),
            ));
            let lock = sbx::webdav::LockInfo::new(
                sbx::webdav::LockScope::Exclusive,
                "opaquelocktoken:t1".into(),
                None,
                std::time::SystemTime::now(),
                None,
                sbx::webdav::Depth::Infinity,
            );
            rt.block_on(async {
                let mut locks = state.locks.write().await;
                locks.insert(dir.path().join("locked_parent"), vec![lock]);
            });
            let router = make_router(state);
            rt.block_on(async {
                let _ = router
                    .oneshot(make_put("/locked_parent/deep.txt", &body_1kb))
                    .await;
            });
        });
    });

    group.finish();
}

criterion_group!(
    middleware,
    bench_health_check,
    bench_auth,
    bench_lock_enforce,
);

criterion_main!(middleware);
