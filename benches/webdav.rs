mod common;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use tempfile::TempDir;
use tower::ServiceExt;

use axum::extract::Request;

use common::*;
use sbx::{AppState, AuthState, make_router};

const ALLPROP_BODY: &[u8] =
    br#"<?xml version="1.0"?><D:propfind xmlns:D="DAV:"><D:allprop/></D:propfind>"#;

fn bench_propfind(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut group = c.benchmark_group("webdav/PROPFIND");

    // Depth:0 on a single file — file created outside iter so the
    // benchmark measures only the PROPFIND request path.
    group.bench_function("depth0_file", |b| {
        let dir = TempDir::new().unwrap();
        create_file(dir.path(), "file.txt", 1024);
        let router = bench_router(dir.path());
        b.iter(|| {
            rt.block_on(async {
                let _ = router
                    .clone()
                    .oneshot(make_propfind("/file.txt", "0", ALLPROP_BODY))
                    .await;
            });
        });
    });

    // Depth:1 on directory — files + router created once per input size,
    // measuring only the PROPFIND dispatch (including batch statx on Linux).
    for count in [10u32, 50, 200, 1000] {
        group.throughput(Throughput::Elements(count as u64));
        group.bench_with_input(
            BenchmarkId::new("depth1_dir", count),
            &count,
            |b, &count| {
                let dir = TempDir::new().unwrap();
                create_small_files(dir.path(), count as usize);
                let router = bench_router(dir.path());
                b.iter(|| {
                    rt.block_on(async {
                        let _ = router
                            .clone()
                            .oneshot(make_propfind("/", "1", ALLPROP_BODY))
                            .await;
                    });
                });
            },
        );
    }

    // Depth:infinity on nested tree — tree built once, router reused.
    group.bench_function("depth_infinity_tree", |b| {
        let dir = TempDir::new().unwrap();
        create_nested_tree(dir.path(), 3, 5);
        let router = bench_router(dir.path());
        b.iter(|| {
            rt.block_on(async {
                let _ = router
                    .clone()
                    .oneshot(make_propfind("/level_0", "infinity", ALLPROP_BODY))
                    .await;
            });
        });
    });

    group.finish();
}

fn bench_mkcol(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("webdav/MKCOL");
    group.bench_function("create_dir", |b| {
        b.iter(|| {
            let dir = TempDir::new().unwrap();
            let router = bench_router(dir.path());
            let name = unique_name("newdir");
            rt.block_on(async {
                let _ = router.oneshot(make_mkcol(&format!("/{name}"))).await;
            });
        });
    });
    group.finish();
}

fn bench_copy(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("webdav/COPY");

    // COPY small file
    group.bench_function("small_file", |b| {
        b.iter(|| {
            let dir = TempDir::new().unwrap();
            create_file(dir.path(), "source.txt", 1024);
            let router = bench_router(dir.path());
            let dest_name = unique_name("dest");
            rt.block_on(async {
                let _ = router
                    .oneshot(make_copy("/source.txt", &format!("/{dest_name}")))
                    .await;
            });
        });
    });

    // COPY directory tree
    group.bench_function("dir_tree", |b| {
        b.iter(|| {
            let dir = TempDir::new().unwrap();
            create_nested_tree(dir.path(), 3, 5);
            let router = bench_router(dir.path());
            let dest_name = unique_name("copytree");
            rt.block_on(async {
                let _ = router
                    .oneshot(make_copy("/level_0", &format!("/{dest_name}")))
                    .await;
            });
        });
    });

    group.finish();
}

fn bench_move(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("webdav/MOVE");

    // MOVE small file (try_rename path)
    group.bench_function("small_file", |b| {
        b.iter(|| {
            let dir = TempDir::new().unwrap();
            create_file(dir.path(), "source.txt", 1024);
            let router = bench_router(dir.path());
            let dest_name = unique_name("movdest");
            rt.block_on(async {
                let _ = router
                    .oneshot(make_move("/source.txt", &format!("/{dest_name}")))
                    .await;
            });
        });
    });

    group.finish();
}

fn bench_lock_unlock(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let lock_body = lock_body_exclusive();
    let shared_body = lock_body_shared();

    let mut group = c.benchmark_group("webdav/LOCK");

    // Acquire exclusive lock
    group.bench_function("acquire_exclusive", |b| {
        b.iter(|| {
            let dir = TempDir::new().unwrap();
            create_file(dir.path(), "target.txt", 1024);
            let router = bench_router(dir.path());
            rt.block_on(async {
                let _ = router.oneshot(make_lock("/target.txt", &lock_body)).await;
            });
        });
    });

    // Acquire shared lock
    group.bench_function("acquire_shared", |b| {
        b.iter(|| {
            let dir = TempDir::new().unwrap();
            create_file(dir.path(), "target.txt", 1024);
            let router = bench_router(dir.path());
            rt.block_on(async {
                let _ = router.oneshot(make_lock("/target.txt", &shared_body)).await;
            });
        });
    });

    // UNLOCK
    group.bench_function("unlock", |b| {
        b.iter(|| {
            let dir = TempDir::new().unwrap();
            create_file(dir.path(), "target.txt", 1024);
            let (router, token) = make_router_with_lock(dir.path());
            rt.block_on(async {
                let _ = router.oneshot(make_unlock("/target.txt", &token)).await;
            });
        });
    });

    group.finish();
}

fn bench_proppatch(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    let body = br#"<?xml version="1.0"?><D:propertyupdate xmlns:D="DAV:"><D:set><D:prop><X:p xmlns="http://example.com/">testval</X:p></D:prop></D:set></D:propertyupdate>"#;

    let mut group = c.benchmark_group("webdav/PROPPATCH");
    group.bench_function("set_property", |b| {
        b.iter(|| {
            let dir = TempDir::new().unwrap();
            create_file(dir.path(), "target.txt", 1024);
            let router = bench_router(dir.path());
            rt.block_on(async {
                let req = Request::builder()
                    .method(axum::http::Method::from_bytes(b"PROPPATCH").unwrap())
                    .uri("/target.txt")
                    .body(axum::body::Body::from(body.to_vec()))
                    .unwrap();
                let _ = router.oneshot(req).await;
            });
        });
    });
    group.finish();
}

fn make_router_with_lock(dir: &std::path::Path) -> (axum::Router, String) {
    let token = sbx::webdav::generate_lock_token();
    let lock = sbx::webdav::LockInfo::new(
        sbx::webdav::LockScope::Exclusive,
        token.clone(),
        None,
        std::time::SystemTime::now(),
        None,
        sbx::webdav::Depth::Zero,
    );
    let state = Arc::new(AppState::new(
        dir.to_path_buf(),
        AuthState::new(),
        Duration::from_secs(300),
    ));
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let mut locks = state.locks.write().await;
        let mut map = HashMap::new();
        map.insert(dir.join("target.txt"), vec![lock]);
        *locks = map;
    });
    (make_router(state), token)
}

criterion_group!(
    webdav,
    bench_propfind,
    bench_mkcol,
    bench_copy,
    bench_move,
    bench_lock_unlock,
    bench_proppatch,
);

criterion_main!(webdav);
