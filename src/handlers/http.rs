//! GET/HEAD (file serving + HTML directory listing), PUT, PATCH, DELETE, and OPTIONS handlers.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use futures_util::TryStreamExt;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
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
        let resp = Response::builder()
            .status(StatusCode::OK)
            .header("content-type", mime.as_ref())
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

/// PATCH handler for partial file updates.
///
/// Supports `X-Update-Range: append` and single `bytes=start-end` ranges.
/// Bytes outside the current file are represented by zeroes. The request
/// content type is not restricted; the update protocol is selected by the
/// PATCH method and its range headers.
///
/// # Errors
///
/// Returns `411 Length Required` when `Content-Length` is absent, `400 Bad
/// Request` for malformed headers, `404 Not Found` for a missing resource, or
/// `416 Range Not Satisfiable` when the body length does not match the range.
pub async fn handle_patch(State(state): State<Arc<AppState>>, req: Request) -> AppResult {
    let content_length = req
        .headers()
        .get(axum::http::header::CONTENT_LENGTH)
        .ok_or(StatusCode::LENGTH_REQUIRED)?
        .to_str()
        .map_err(|_| StatusCode::BAD_REQUEST)?
        .parse::<u64>()
        .map_err(|_| StatusCode::BAD_REQUEST)?;

    let range = req
        .headers()
        .get("x-update-range")
        .ok_or(StatusCode::BAD_REQUEST)?
        .to_str()
        .map_err(|_| StatusCode::BAD_REQUEST)
        .and_then(parse_patch_range)?;

    let request_path = req.uri().path().to_owned();
    let target = state.resolve_and_guard(&request_path).await;
    let target = target.or_invalid(StatusCode::BAD_REQUEST)?;
    let metadata = tokio::fs::metadata(&target)
        .await
        .or_404("resource not found for PATCH")?;
    if !metadata.is_file() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let body = axum::body::to_bytes(req.into_body(), usize::MAX)
        .await
        .map_err(|_| StatusCode::RANGE_NOT_SATISFIABLE)?;
    if body.len() as u64 != content_length {
        return Err(StatusCode::RANGE_NOT_SATISFIABLE);
    }

    let file_size = metadata.len();
    let (offset, end, suffix_len) = match range {
        PatchRange::Append => (file_size, None, None),
        PatchRange::Bytes { start, end } => {
            let (offset, suffix_len) = match start {
                PatchStart::Positive(value) => (value, None),
                PatchStart::Negative(value) => (file_size.saturating_sub(value), Some(value)),
            };
            (offset, end, suffix_len)
        }
    };

    let body_len = body.len() as u64;
    if let Some(suffix_len) = suffix_len
        && suffix_len != body_len
    {
        return Err(StatusCode::RANGE_NOT_SATISFIABLE);
    }
    if let Some(end) = end {
        let range_len = end
            .checked_sub(offset)
            .and_then(|length| length.checked_add(1));
        if range_len != Some(body_len) {
            return Err(StatusCode::RANGE_NOT_SATISFIABLE);
        }
    }

    let write_end = offset
        .checked_add(body_len)
        .ok_or(StatusCode::RANGE_NOT_SATISFIABLE)?;
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .open(&target)
        .await
        .or_500("failed to open file for PATCH")?;

    if offset > file_size {
        file.set_len(offset)
            .await
            .or_500("failed to create PATCH zero-fill gap")?;
    }
    file.seek(std::io::SeekFrom::Start(offset))
        .await
        .or_500("failed to seek for PATCH")?;
    file.write_all(&body)
        .await
        .or_500("failed to write PATCH body")?;
    file.flush().await.or_500("failed to flush PATCH file")?;

    tracing::debug!(
        path = %target.display(), offset, end = ?end, size = body_len, final_end = write_end,
        "PATCH completed"
    );
    Ok(StatusCode::NO_CONTENT.into_response())
}

enum PatchRange {
    Append,
    Bytes { start: PatchStart, end: Option<u64> },
}

enum PatchStart {
    Positive(u64),
    Negative(u64),
}

fn parse_patch_range(value: &str) -> Result<PatchRange, StatusCode> {
    if value.trim() == "append" {
        return Ok(PatchRange::Append);
    }

    let range = value
        .strip_prefix("bytes=")
        .ok_or(StatusCode::BAD_REQUEST)?;
    if range.is_empty() || range.contains(',') {
        return Err(StatusCode::BAD_REQUEST);
    }

    if let Some(suffix) = range.strip_prefix('-') {
        if suffix.is_empty() {
            return Err(StatusCode::BAD_REQUEST);
        }
        return Ok(PatchRange::Bytes {
            start: PatchStart::Negative(
                suffix.parse::<u64>().map_err(|_| StatusCode::BAD_REQUEST)?,
            ),
            end: None,
        });
    }

    let (start, end) = range.split_once('-').ok_or(StatusCode::BAD_REQUEST)?;
    let start = PatchStart::Positive(start.parse::<u64>().map_err(|_| StatusCode::BAD_REQUEST)?);
    let end = if end.is_empty() {
        None
    } else {
        Some(end.parse::<u64>().map_err(|_| StatusCode::BAD_REQUEST)?)
    };

    Ok(PatchRange::Bytes { start, end })
}

/// OPTIONS handler — returns supported HTTP/DAV methods in the `Allow` header.
///
/// Includes the `DAV: 1, 2` compliance level and `MS-Author-Via: DAV`
/// header for compatibility with legacy clients.
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
            "GET, HEAD, OPTIONS, PUT, PATCH, DELETE, PROPFIND, MKCOL, COPY, MOVE, PROPPATCH, LOCK, UNLOCK",
        )
        .header("dav", "1, 2")
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
        assert!(allow.contains("PATCH"));
        assert!(allow.contains("DELETE"));
        assert!(allow.contains("PROPFIND"));
        assert!(allow.contains("MKCOL"));
        assert_eq!(resp.headers().get("dav").unwrap().to_str().unwrap(), "1, 2");
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
