//! GET/HEAD (file serving + HTML directory listing), PUT, DELETE, and OPTIONS handlers.

use std::sync::Arc;
use std::time::UNIX_EPOCH;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use futures_util::TryStreamExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::io::{ReaderStream, StreamReader};

use crate::html::generate_dir_listing;
use crate::server::{AppResult, AppState};
use crate::utils::error::{IntoResolved, OrStatus};

/// GET / HEAD handler — serves files and generates HTML directory listings.
///
/// Supports conditional `If-Modified-Since` via the `Last-Modified` header.
/// Accepts `Range` requests for partial content delivery.
///
/// # Panics
///
/// Panics if constructing an HTTP response fails. In practice this never
/// occurs — the response builder is always fresh and the body/headers are
/// valid.
pub async fn handle_get_head(State(state): State<Arc<AppState>>, req: Request) -> AppResult {
    let request_path = req.uri().path().to_owned();

    let fs_path = state.resolve_existing(&request_path).await;
    let fs_path = fs_path.or_404("path resolution failed")?;

    let meta = tokio::fs::metadata(&fs_path).await;
    let meta = meta.or_404("metadata failed for GET/HEAD")?;

    let method = req.method();
    if meta.is_dir() {
        let (html, entry_count) = generate_dir_listing(&fs_path, &request_path).await;
        tracing::debug!(entry_count = entry_count, "directory listing");
        let resp = Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "text/html; charset=utf-8")
            .header("content-length", html.len());
        if method == axum::http::Method::HEAD {
            return Ok(resp.body(Body::empty()).unwrap());
        }
        Ok(resp.body(Body::from(html)).unwrap())
    } else {
        let file_size = meta.len();
        let mime = mime_guess::from_path(&fs_path).first_or_octet_stream();
        tracing::debug!(mime = %mime.essence_str(), size = file_size, "file served");
        let is_text = mime.essence_str().starts_with("text/");
        let range = req.headers().get("range").and_then(|v| v.to_str().ok());
        let line_range = range.and_then(parse_line_range_request);
        if method == axum::http::Method::GET
            && let Some(result) = line_range
        {
            if !is_text {
                return Ok(range_not_satisfiable(0, false));
            }
            let mut file = tokio::fs::File::open(&fs_path)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            let snapshot_meta = file
                .metadata()
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            let mut data = Vec::with_capacity(snapshot_meta.len().min(usize::MAX as u64) as usize);
            file.read_to_end(&mut data)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            let end_meta = file
                .metadata()
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            if end_meta.len() != snapshot_meta.len()
                || end_meta.modified().unwrap_or(UNIX_EPOCH)
                    != snapshot_meta.modified().unwrap_or(UNIX_EPOCH)
                || data.len() as u64 != snapshot_meta.len()
            {
                return Err(StatusCode::INTERNAL_SERVER_ERROR);
            }
            let lines = line_offsets(&data);
            let (start, requested_end) = match result {
                Ok(range) => range,
                Err(()) => return Ok(range_not_satisfiable(0, false)),
            };
            if start == 0 || start > lines.len() {
                return Ok(range_not_satisfiable(lines.len(), true));
            }
            let end = requested_end.unwrap_or(lines.len()).min(lines.len());
            if end < start {
                return Ok(range_not_satisfiable(0, false));
            }
            let (lo, hi) = lines[start - 1..end]
                .iter()
                .fold((usize::MAX, 0), |(lo, hi), (a, b)| (lo.min(*a), hi.max(*b)));
            let body = &data[lo..hi];
            let etag = make_etag(&snapshot_meta);
            let last_modified = snapshot_meta
                .modified()
                .map(crate::utils::time::format_rfc1123)
                .unwrap_or_default();
            if let Some(if_range) = req.headers().get("if-range").and_then(|v| v.to_str().ok())
                && if_range != etag
                && if_range != last_modified
            {
                return Ok(Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", mime.as_ref())
                    .header(
                        "accept-ranges",
                        if is_text { "bytes, lines" } else { "bytes" },
                    )
                    .header("etag", etag)
                    .header("last-modified", last_modified)
                    .header("content-length", data.len())
                    .body(Body::from(data))
                    .unwrap());
            }
            return Ok(Response::builder()
                .status(StatusCode::PARTIAL_CONTENT)
                .header("content-type", mime.as_ref())
                .header("accept-ranges", "bytes, lines")
                .header("etag", etag)
                .header("last-modified", last_modified)
                .header(
                    "content-range",
                    format!("lines {start}-{end}/{}", lines.len()),
                )
                .header("content-length", body.len())
                .body(Body::from(body.to_vec()))
                .unwrap());
        }
        let resp = Response::builder()
            .status(StatusCode::OK)
            .header("content-type", mime.as_ref())
            .header(
                "accept-ranges",
                if is_text { "bytes, lines" } else { "bytes" },
            )
            .header("etag", make_etag(&meta))
            .header(
                "last-modified",
                meta.modified()
                    .map(crate::utils::time::format_rfc1123)
                    .unwrap_or_default(),
            )
            .header("content-length", file_size);
        if method == axum::http::Method::HEAD {
            return Ok(resp.body(Body::empty()).unwrap());
        }
        match tokio::fs::File::open(&fs_path).await {
            Ok(file) => {
                let stream = ReaderStream::new(file);
                Ok(resp.body(Body::from_stream(stream)).unwrap())
            }
            Err(e) => {
                tracing::error!(error = %e, "open failed");
                Err(StatusCode::INTERNAL_SERVER_ERROR)
            }
        }
    }
}

fn parse_line_range_request(value: &str) -> Option<Result<(usize, Option<usize>), ()>> {
    let segments: Vec<_> = value.split(',').collect();
    let has_lines = segments.iter().any(|segment| {
        let unit = segment.split_once('=').map_or(*segment, |(unit, _)| unit);
        unit.trim().eq_ignore_ascii_case("lines")
    });
    if !has_lines {
        return None;
    }
    if segments.len() != 1 {
        return Some(Err(()));
    }
    let (unit, rest) = match value.split_once('=') {
        Some(parts) => parts,
        None => return Some(Err(())),
    };
    if unit != "lines" && !unit.eq_ignore_ascii_case("lines") {
        return Some(Err(()));
    }
    if unit.trim() != unit {
        return Some(Err(()));
    }
    let mut parts = rest.split('-');
    let start = parts
        .next()
        .filter(|part| !part.is_empty())
        .and_then(|part| part.parse::<usize>().ok())
        .filter(|&part| part > 0);
    let end = match parts.next() {
        Some("") => Some(None),
        Some(part) => part.parse::<usize>().ok().map(Some),
        None => None,
    };
    if parts.next().is_some() {
        return Some(Err(()));
    }
    match (start, end) {
        (Some(start), Some(end)) => Some(Ok((start, end))),
        _ => Some(Err(())),
    }
}

fn range_not_satisfiable(total: usize, include_total: bool) -> Response {
    let mut response = Response::builder().status(StatusCode::RANGE_NOT_SATISFIABLE);
    if include_total {
        response = response.header("content-range", format!("lines */{total}"));
    }
    response.body(Body::empty()).unwrap()
}

fn make_etag(meta: &std::fs::Metadata) -> String {
    format!(
        "\"{:x}-{:x}\"",
        meta.modified()
            .unwrap_or(UNIX_EPOCH)
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        meta.len()
    )
}

fn line_offsets(data: &[u8]) -> Vec<(usize, usize)> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < data.len() {
        if data[i] == b'\n' || data[i] == b'\r' {
            let end = if data[i] == b'\r' && data.get(i + 1) == Some(&b'\n') {
                i + 2
            } else {
                i + 1
            };
            lines.push((start, end));
            start = end;
            i = end;
        } else {
            i += 1;
        }
    }
    if start < data.len() {
        lines.push((start, data.len()));
    }
    lines
}

/// PUT handler — accepts a request body and writes it to the filesystem.
///
/// Returns `201 Created` for new files, `200 OK` for overwrites.
/// Rejects directory paths, missing parents, and traversal attempts.
/// Intermediate collections are NOT created (per RFC 4918 §9.6).
pub async fn handle_put(State(state): State<Arc<AppState>>, req: Request) -> AppResult {
    let request_path = req.uri().path().to_owned();

    // PUT MUST NOT create intermediate collections (RFC 4918 §9.6)
    let target = state.resolve_and_guard(&request_path).await;
    let target = target.or_invalid(StatusCode::BAD_REQUEST)?;

    let existed = tokio::fs::try_exists(&target).await.unwrap_or(false);
    let mut file = match tokio::fs::File::create(&target).await {
        Ok(f) => f,
        Err(e) => {
            tracing::error!(
                error = %e, path = %target.display(), "failed to create file for PUT"
            );
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    let body = req.into_body();
    let stream = body.into_data_stream().map_err(std::io::Error::other);
    let mut reader = StreamReader::new(stream);

    let bytes_written = match tokio::io::copy(&mut reader, &mut file).await {
        Ok(n) => n,
        Err(e) => {
            tracing::error!(
                error = %e, path = %target.display(), "error writing PUT body"
            );
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    if let Err(e) = file.flush().await {
        tracing::error!(error = %e, path = %target.display(), "error flushing PUT file");
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

    tracing::debug!(
        path = %target.display(), size = bytes_written, existed = existed, "PUT completed"
    );

    if existed {
        Ok(StatusCode::OK.into_response())
    } else {
        Ok(StatusCode::CREATED.into_response())
    }
}

/// DELETE handler — removes a file or recursively deletes a directory.
///
/// Returns `204 No Content` on success, `404 Not Found` if the target
/// does not exist. Root directory deletion is rejected.
pub async fn handle_delete(State(state): State<Arc<AppState>>, req: Request) -> AppResult {
    let request_path = req.uri().path().to_owned();

    let fs_path = state.resolve_existing(&request_path).await;
    let fs_path = fs_path.or_404("path resolution failed for DELETE")?;

    let meta = tokio::fs::metadata(&fs_path).await;
    let meta = meta.or_404("metadata failed for DELETE")?;

    if meta.is_dir() {
        if fs_path == state.root_canonical {
            tracing::debug!("DELETE rejected: root directory");
            return Err(StatusCode::BAD_REQUEST);
        }
        match tokio::fs::remove_dir_all(&fs_path).await {
            Ok(()) => {
                tracing::debug!(path = %fs_path.display(), "DELETE directory completed");
                Ok(StatusCode::NO_CONTENT.into_response())
            }
            Err(e) => {
                tracing::error!(
                    error = %e, path = %fs_path.display(), "failed to remove directory for DELETE"
                );
                Err(StatusCode::INTERNAL_SERVER_ERROR)
            }
        }
    } else {
        match tokio::fs::remove_file(&fs_path).await {
            Ok(()) => {
                tracing::debug!(path = %fs_path.display(), "DELETE completed");
                Ok(StatusCode::NO_CONTENT.into_response())
            }
            Err(e) => {
                tracing::error!(
                    error = %e, path = %fs_path.display(), "failed to remove file for DELETE"
                );
                Err(StatusCode::INTERNAL_SERVER_ERROR)
            }
        }
    }
}

/// OPTIONS handler — returns supported HTTP/DAV methods in the `Allow` header.
///
/// Includes the `DAV: 1,2` compliance level and `MS-Author-Via: DAV` header
/// for compatibility with legacy clients.
///
/// # Panics
///
/// Panics if constructing the response fails. In practice this never occurs
/// — the response builder is always fresh with valid headers and an empty body.
pub async fn handle_options() -> AppResult {
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(
            "allow",
            "GET, HEAD, OPTIONS, PUT, DELETE, PROPFIND, MKCOL, COPY, MOVE, PROPPATCH, LOCK, UNLOCK",
        )
        .header("dav", "1,2")
        .header("ms-author-via", "DAV")
        .header("content-length", "0")
        .body(Body::empty())
        .unwrap())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {

    use std::sync::Arc;

    use axum::{Router, body::Body, extract::Request};
    use tower::ServiceExt;

    use crate::{AppState, AuthState};

    // -- GET/HEAD tests ------------------------------------------------------

    fn make_app_get(dir: &tempfile::TempDir) -> Router {
        Router::new()
            .fallback(super::handle_get_head)
            .with_state(Arc::new(AppState::new(
                dir.path().to_path_buf(),
                AuthState::new(),
                std::time::Duration::from_secs(300),
            )))
    }

    fn setup_get_test_dir() -> tempfile::TempDir {
        use std::io::Write;
        let dir = tempfile::TempDir::new().unwrap();
        let mut f = std::fs::File::create(dir.path().join("hello.txt")).unwrap();
        f.write_all(b"Hello, World!").unwrap();
        std::fs::create_dir(dir.path().join("subdir")).unwrap();
        let mut f = std::fs::File::create(dir.path().join("subdir/nested.txt")).unwrap();
        f.write_all(b"Nested file").unwrap();
        dir
    }

    #[tokio::test]
    async fn test_get_path_traversal_blocked() {
        let dir = setup_get_test_dir();
        let app = make_app_get(&dir);

        let req = Request::builder()
            .method(axum::http::Method::GET)
            .uri("/../outside.txt")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_get_mime_type_guess() {
        let dir = setup_get_test_dir();
        let app = make_app_get(&dir);

        let req = Request::builder()
            .method(axum::http::Method::GET)
            .uri("/hello.txt")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert!(
            resp.headers()
                .get("content-type")
                .unwrap()
                .to_str()
                .unwrap()
                .contains("text/plain")
        );
    }

    #[tokio::test]
    async fn test_get_dir_listing_sizes() {
        let dir = setup_get_test_dir();
        let app = make_app_get(&dir);

        let req = Request::builder()
            .method(axum::http::Method::GET)
            .uri("/")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = String::from_utf8(body.to_vec()).unwrap();

        assert!(body_str.contains("hello.txt") && body_str.contains("13"));
        assert!(body_str.contains("subdir/") && body_str.contains("-"));
    }

    #[tokio::test]
    async fn test_get_line_range_preserves_line_endings() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("lines.txt"), b"one\r\ntwo\rthree\nfour").unwrap();
        let app = make_app_get(&dir);
        let req = Request::builder()
            .method(axum::http::Method::GET)
            .uri("/lines.txt")
            .header("range", "lines=2-3")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::PARTIAL_CONTENT);
        assert_eq!(resp.headers()["content-range"], "lines 2-3/4");
        assert_eq!(
            axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap(),
            &b"two\rthree\n"[..]
        );
    }

    #[tokio::test]
    async fn test_get_line_range_clips_and_rejects_invalid_requests() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("lines.txt"), b"one\ntwo\nthree").unwrap();
        std::fs::write(dir.path().join("data.bin"), [0, 1, 2, 3]).unwrap();

        let app = make_app_get(&dir);
        let req = Request::builder()
            .method(axum::http::Method::GET)
            .uri("/lines.txt")
            .header("range", "lines=2-999")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::PARTIAL_CONTENT);
        assert_eq!(resp.headers()["content-range"], "lines 2-3/3");

        for range in ["lines=-1", "lines=4-5", "bytes=0-1,lines=1-2"] {
            let req = Request::builder()
                .method(axum::http::Method::GET)
                .uri("/lines.txt")
                .header("range", range)
                .body(Body::empty())
                .unwrap();
            let resp = app.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status(), axum::http::StatusCode::RANGE_NOT_SATISFIABLE);
        }

        let req = Request::builder()
            .method(axum::http::Method::GET)
            .uri("/data.bin")
            .header("range", "lines=1-1")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::RANGE_NOT_SATISFIABLE);
        assert!(resp.headers().get("content-range").is_none());
        assert!(
            axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn test_head_ignores_line_range_and_if_range() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("lines.txt"), b"one\ntwo\nthree").unwrap();
        let app = make_app_get(&dir);
        let req = Request::builder()
            .method(axum::http::Method::HEAD)
            .uri("/lines.txt")
            .header("range", "lines=2-2")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        assert_eq!(resp.headers()["content-length"], "13");
        assert!(
            axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn test_if_range_mismatch_returns_full_representation() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("lines.txt"), b"one\ntwo\nthree").unwrap();
        let app = make_app_get(&dir);
        let req = Request::builder()
            .method(axum::http::Method::GET)
            .uri("/lines.txt")
            .header("range", "lines=2-2")
            .header("if-range", "\"stale\"")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        assert_eq!(
            axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap(),
            &b"one\ntwo\nthree"[..]
        );
    }

    // -- PUT tests -----------------------------------------------------------

    fn make_app_put(dir: &tempfile::TempDir) -> Router {
        Router::new()
            .route("/", axum::routing::put(super::handle_put))
            .route("/{*path}", axum::routing::put(super::handle_put))
            .with_state(Arc::new(AppState::new(
                dir.path().to_path_buf(),
                AuthState::new(),
                std::time::Duration::from_secs(300),
            )))
    }

    #[tokio::test]
    async fn test_put_rejects_missing_parent() {
        let dir = tempfile::TempDir::new().unwrap();
        let app = make_app_put(&dir);

        let req = Request::builder()
            .method(axum::http::Method::PUT)
            .uri("/a/b/c/file.txt")
            .body(Body::from("nested"))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        // RFC 4918 §9.6: PUT MUST NOT create intermediate collections
        assert_eq!(resp.status(), axum::http::StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn test_put_rejects_root_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let app = make_app_put(&dir);

        let req = Request::builder()
            .method(axum::http::Method::PUT)
            .uri("/")
            .body(Body::from("bad"))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_put_rejects_dir_path() {
        let dir = tempfile::TempDir::new().unwrap();
        let app = make_app_put(&dir);

        let req = Request::builder()
            .method(axum::http::Method::PUT)
            .uri("/subdir/")
            .body(Body::from("bad"))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_put_rejects_traversal() {
        let dir = tempfile::TempDir::new().unwrap();
        let app = make_app_put(&dir);

        let req = Request::builder()
            .method(axum::http::Method::PUT)
            .uri("/../outside.txt")
            .body(Body::from("bad"))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::BAD_REQUEST);
    }

    // -- DELETE tests --------------------------------------------------------

    fn make_app_delete(dir: &tempfile::TempDir) -> Router {
        Router::new()
            .route("/", axum::routing::delete(super::handle_delete))
            .route("/{*path}", axum::routing::delete(super::handle_delete))
            .with_state(Arc::new(AppState::new(
                dir.path().to_path_buf(),
                AuthState::new(),
                std::time::Duration::from_secs(300),
            )))
    }

    #[tokio::test]
    async fn test_delete_root_rejected() {
        let dir = tempfile::TempDir::new().unwrap();
        let app = make_app_delete(&dir);

        let req = Request::builder()
            .method(axum::http::Method::DELETE)
            .uri("/")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_delete_dir_trailing_slash() {
        let dir = tempfile::TempDir::new().unwrap();
        let subdir = dir.path().join("mydir");
        std::fs::create_dir(&subdir).unwrap();
        assert!(subdir.exists());
        let app = make_app_delete(&dir);

        let req = Request::builder()
            .method(axum::http::Method::DELETE)
            .uri("/mydir/")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::NO_CONTENT);
        assert!(!subdir.exists());
    }

    #[tokio::test]
    async fn test_delete_rejects_traversal() {
        let dir = tempfile::TempDir::new().unwrap();
        let app = make_app_delete(&dir);

        let req = Request::builder()
            .method(axum::http::Method::DELETE)
            .uri("/../outside.txt")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND);
    }

    // -- OPTIONS tests -------------------------------------------------------

    #[tokio::test]
    async fn test_options_returns_ok() {
        let resp = super::handle_options().await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let allow = resp.headers().get("allow").unwrap().to_str().unwrap();
        assert!(allow.contains("GET"));
        assert!(allow.contains("PUT"));
        assert!(allow.contains("DELETE"));
        assert!(allow.contains("PROPFIND"));
        assert!(allow.contains("MKCOL"));
        assert_eq!(resp.headers().get("dav").unwrap().to_str().unwrap(), "1,2");
        assert_eq!(
            resp.headers()
                .get("ms-author-via")
                .unwrap()
                .to_str()
                .unwrap(),
            "DAV"
        );
    }

    #[tokio::test]
    async fn test_options_has_content_length_zero() {
        let resp = super::handle_options().await.unwrap();
        let cl = resp
            .headers()
            .get("content-length")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(cl, "0");
    }

    #[tokio::test]
    async fn test_options_body_empty() {
        let resp = super::handle_options().await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(body.is_empty());
    }
}
