//! The file API endpoints.
//!
//! Files map directly to URIs under the workspace root, and every file URI shares
//! one [`MethodRouter`], so axum supplies `405 Method Not Allowed` with its `Allow`
//! header. Operations that WebDAV expresses as extensions to the method itself are
//! submitted as a `POST` body instead ([`FileOperation`]), because axum routes only
//! standard HTTP methods.
//!
//! One deviation remains: rejections raised by axum's own extractors, such as
//! [`Path`] or [`Json`], still return axum's plain text error body instead of the
//! envelope. The extractors defined in this module reject with [`AppError`].

use std::{
    collections::HashMap,
    convert::Infallible,
    path::{Component, Path, PathBuf},
};

use super::file::{
    FileContent, FileHeaders, INLINE_BODY_LIMIT, ReadFileError, prepare_download, probe_file,
};
use super::model::{FileOperation, FilePath};
use crate::workspace::registry::TargetFile;
use crate::{AppError, AppState};
use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::{FromRequestParts, OriginalUri, Path as AxumPath, RawQuery, State};
use axum::http::Method;
use axum::http::header::{self, HeaderValue};
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use axum::routing::{self, MethodRouter};
use tokio_stream::StreamExt;

/// Build the file router.
///
/// Paths include the owning workspace because a file is always addressed
/// within one, but the routes stand on their own rather than being nested into
/// the workspace control plane.
pub(crate) fn router() -> Router<AppState> {
    // A wildcard capture never matches an empty path, and `/fs` and `/fs/`
    // are distinct routes in axum, so addressing the workspace root needs these
    // two routes; the wildcard route matches neither of them.
    Router::new()
        .route("/workspaces/{workspace_id}/fs", file_routes())
        .route("/workspaces/{workspace_id}/fs/", file_routes())
        .route("/workspaces/{workspace_id}/fs/{*path}", file_routes())
        .route("/fs", file_routes())
        .route("/fs/", file_routes())
        .route("/fs/{*path}", file_routes())
}

/// Route table shared by the workspace root and every file below it.
fn file_routes() -> MethodRouter<AppState> {
    routing::get(get_file)
        .head(head_file)
        .post(post_file)
        .put(put_file)
        .delete(delete_file)
        .fallback(method_not_allowed)
}

struct FileTarget(TargetFile);

impl FromRequestParts<AppState> for FileTarget {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let AxumPath(captures): AxumPath<HashMap<String, String>> =
            AxumPath::from_request_parts(parts, state)
                .await
                .map_err(|_| AppError::BadRequest("invalid file path".to_owned()))?;
        let path = captures.get("path").map(String::as_str).unwrap_or("");
        let path = validate_path(path)?;

        let target_file = if let Some(workspace_id) = captures.get("workspace_id") {
            if path.as_os_str().is_empty() {
                return Err(AppError::BadRequest(
                    "file path must not be empty".to_owned(),
                ));
            }
            state.workspaces().target_file(workspace_id, path)?
        } else {
            if path.as_os_str().is_empty() {
                return Err(AppError::BadRequest(
                    "file path must not be empty".to_owned(),
                ));
            }
            TargetFile::Absolute(PathBuf::from("/").join(path))
        };

        Ok(Self(target_file))
    }
}

/// Reject the `type` query parameter on data-plane requests.
///
/// `type` selects a control-plane operation, which `QUERY`, `PATCH` and `POST`
/// carry; on `GET` and `HEAD` the combination is `405 method_not_allowed`.
fn reject_type(RawQuery(query): &RawQuery) -> Result<(), AppError> {
    let has_type = query.as_deref().is_some_and(|query| {
        url::form_urlencoded::parse(query.as_bytes()).any(|(key, _)| key == "type")
    });
    if has_type {
        return Err(AppError::MethodNotAllowed(Method::GET));
    }

    Ok(())
}

/// A data-plane request must carry no body, so any data frame rejects it.
async fn ensure_empty_body(body: Body) -> Result<(), AppError> {
    let mut stream = body.into_data_stream();
    while let Some(frame) = stream.next().await {
        let frame = frame.map_err(|error| {
            AppError::BadRequest(format!("failed to read request body: {error}"))
        })?;
        if !frame.is_empty() {
            return Err(AppError::BadRequest(
                "request body must be empty".to_owned(),
            ));
        }
    }

    Ok(())
}

fn validate_path(value: &str) -> Result<PathBuf, AppError> {
    if value.as_bytes().contains(&0) {
        return Err(AppError::BadRequest("path must not contain NUL".to_owned()));
    }
    let path = Path::new(value);
    if value.starts_with('/') {
        return Err(AppError::BadRequest(
            "workspace path must be relative".to_owned(),
        ));
    }
    if path.components().any(|component| {
        matches!(
            component,
            Component::CurDir | Component::ParentDir | Component::RootDir
        )
    }) {
        return Err(AppError::BadRequest("path must be normalized".to_owned()));
    }
    Ok(path.to_owned())
}

fn map_read_error(error: ReadFileError) -> AppError {
    match error {
        ReadFileError::NotFound(path) => AppError::NotFound(path),
        ReadFileError::NotAFile(path) => AppError::NotAFile(path),
        ReadFileError::InvalidFile(message) => AppError::BadRequest(message),
        ReadFileError::Io(error) => AppError::Internal(error.into()),
    }
}

impl FileHeaders {
    /// Stamp the data-plane headers onto a response built from the content.
    ///
    /// Inserting after the body is built matters: `IntoResponse` for bytes
    /// guesses `Content-Type` from the payload, and only a real `HeaderMap`
    /// insert reliably overrides that guess.
    fn stamp_on(&self, response: &mut Response) {
        let headers = response.headers_mut();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_str(self.content_type.as_ref())
                .expect("a MIME type is a valid header value"),
        );
        headers.insert(
            header::CONTENT_LENGTH,
            HeaderValue::from(self.content_length),
        );
        headers.insert(
            header::ETAG,
            HeaderValue::from_str(&self.etag).expect("an entity-tag is a valid header value"),
        );
        headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("none"));
        headers.insert(header::LAST_MODIFIED, self.last_modified());
    }

    fn last_modified(&self) -> HeaderValue {
        self.last_modified
            .map(|time| HeaderValue::from_str(&httpdate::fmt_http_date(time)))
            .transpose()
            .expect("an HTTP date is a valid header value")
            .unwrap_or_else(|| HeaderValue::from_static("Thu, 01 Jan 1970 00:00:00 GMT"))
    }
}

/// Conditional request headers shared by file mutations.
///
/// The values are kept verbatim: interpreting `ETag` lists, and deciding which
/// precondition status a failure maps to, is the handler's job.
#[derive(Debug, Default)]
pub(super) struct ConditionalHeaders {
    #[expect(dead_code, reason = "read by the resource handlers, which are stubs")]
    if_match: Option<HeaderValue>,
    #[expect(dead_code, reason = "read by the resource handlers, which are stubs")]
    if_none_match: Option<HeaderValue>,
}

impl<S> FromRequestParts<S> for ConditionalHeaders
where
    S: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let headers = &parts.headers;

        Ok(Self {
            if_match: headers.get(header::IF_MATCH).cloned(),
            if_none_match: headers.get(header::IF_NONE_MATCH).cloned(),
        })
    }
}

/// The response shape depends on the target file, which is why this handler
/// returns [`Response`] rather than a concrete JSON type.
async fn get_file(
    State(_state): State<AppState>,
    target: FileTarget,
    query: RawQuery,
    OriginalUri(_original_uri): OriginalUri,
    body: Body,
) -> Result<Response, AppError> {
    reject_type(&query)?;
    ensure_empty_body(body).await?;

    let prepared = prepare_download(&target.0, INLINE_BODY_LIMIT)
        .await
        .map_err(map_read_error)?;

    let mut response = match prepared.content {
        FileContent::Inline(bytes) => bytes.into_response(),
        FileContent::Stream(stream) => Body::from_stream(stream).into_response(),
    };
    prepared.headers.stamp_on(&mut response);

    Ok(response)
}

/// `HEAD` runs the same validation as `GET` but answers headers only.
async fn head_file(
    State(_state): State<AppState>,
    target: FileTarget,
    query: RawQuery,
    OriginalUri(_original_uri): OriginalUri,
    body: Body,
) -> Result<Response, AppError> {
    reject_type(&query)?;
    ensure_empty_body(body).await?;

    let headers = probe_file(&target.0).await.map_err(map_read_error)?;

    let mut response = Body::empty().into_response();
    headers.stamp_on(&mut response);

    Ok(response)
}

async fn post_file(
    State(_state): State<AppState>,
    AxumPath(_file): AxumPath<FilePath>,
    _conditions: ConditionalHeaders,
    OriginalUri(_original_uri): OriginalUri,
    Json(_operation): Json<FileOperation>,
) -> Result<Response, AppError> {
    Err(AppError::NotImplemented("POST file"))
}

async fn put_file(
    State(_state): State<AppState>,
    AxumPath(_file): AxumPath<FilePath>,
    _conditions: ConditionalHeaders,
    OriginalUri(_original_uri): OriginalUri,
    _body: Body,
) -> Result<Response, AppError> {
    Err(AppError::NotImplemented("PUT file"))
}

async fn delete_file(
    State(_state): State<AppState>,
    AxumPath(_file): AxumPath<FilePath>,
    _conditions: ConditionalHeaders,
    OriginalUri(_original_uri): OriginalUri,
) -> Result<Response, AppError> {
    Err(AppError::NotImplemented("DELETE file"))
}

/// Fallback for the methods a file URI does not support.
///
/// axum answers those with `405 Method Not Allowed` on its own, but with an empty
/// body; going through a fallback keeps the JSON error envelope. The `Allow`
/// header is still added by axum, because every method this fallback covers is
/// absent from the [`MethodRouter`].
async fn method_not_allowed(method: Method) -> AppError {
    AppError::MethodNotAllowed(method)
}

#[cfg(test)]
mod tests {

    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use axum::response::Response;
    use tower::ServiceExt;

    use crate::AppState;
    use crate::workspace::registry::test_support::TempDir;

    use super::router;

    async fn call(app: &Router, request: Request<Body>) -> Response {
        app.clone().oneshot(request).await.unwrap()
    }

    #[tokio::test]
    async fn get_serves_a_file_with_data_plane_headers() {
        let dir = TempDir::new();
        tokio::fs::write(dir.path().join("note.txt"), "hello")
            .await
            .unwrap();
        let path = dir.path().join("note.txt");
        let uri = format!("/fs{}", path.display());
        let app = router().with_state(AppState::new("."));

        let response = call(
            &app,
            Request::builder().uri(&uri).body(Body::empty()).unwrap(),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        let headers = response.headers();
        assert_eq!(headers.get(header::CONTENT_TYPE).unwrap(), "text/plain");
        assert_eq!(headers.get(header::CONTENT_LENGTH).unwrap(), "5");
        assert_eq!(headers.get(header::ACCEPT_RANGES).unwrap(), "none");
        assert!(headers.contains_key(header::ETAG));
        assert!(headers.contains_key(header::LAST_MODIFIED));

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"hello");
    }

    #[tokio::test]
    async fn head_returns_the_same_headers_without_a_body() {
        let dir = TempDir::new();
        tokio::fs::write(dir.path().join("note.txt"), "hello")
            .await
            .unwrap();
        let uri = format!("/fs{}", dir.path().join("note.txt").display());
        let app = router().with_state(AppState::new("."));

        let response = call(
            &app,
            Request::builder()
                .method("HEAD")
                .uri(&uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        let headers = response.headers();
        assert_eq!(headers.get(header::CONTENT_LENGTH).unwrap(), "5");
        assert_eq!(headers.get(header::CONTENT_TYPE).unwrap(), "text/plain");
        assert!(headers.contains_key(header::ETAG));
        assert!(headers.contains_key(header::LAST_MODIFIED));

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(body.is_empty());
    }

    #[tokio::test]
    async fn get_rejects_a_type_parameter_a_body_and_a_directory() {
        let dir = TempDir::new();
        tokio::fs::write(dir.path().join("note.txt"), "hello")
            .await
            .unwrap();
        let file_uri = format!("/fs{}", dir.path().join("note.txt").display());
        let app = router().with_state(AppState::new("."));

        let response = call(
            &app,
            Request::builder()
                .uri(format!("{file_uri}?type=metadata"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);

        let response = call(
            &app,
            Request::builder()
                .uri(&file_uri)
                .header(header::CONTENT_LENGTH, "3")
                .body(Body::from("abc"))
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let response = call(
            &app,
            Request::builder()
                .uri(format!("/fs{}", dir.path().display()))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["error"]["code"].as_str(), Some("not_a_file"));

        let response = call(
            &app,
            Request::builder()
                .uri(format!("/fs{}", dir.path().join("missing.txt").display()))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn get_serves_a_file_through_a_workspace() {
        let dir = TempDir::new();
        tokio::fs::write(dir.path().join("note.txt"), "hello")
            .await
            .unwrap();
        let state = AppState::new(".");
        state
            .workspaces()
            .register("docs", &dir.root())
            .await
            .unwrap();
        let app = router().with_state(state);

        let response = call(
            &app,
            Request::builder()
                .uri("/workspaces/docs/fs/note.txt")
                .body(Body::empty())
                .unwrap(),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"hello");
    }

    #[tokio::test]
    async fn rejects_workspace_paths_that_escape_the_root() {
        let dir = TempDir::new();
        let state = AppState::new(".");
        state
            .workspaces()
            .register("docs", &dir.root())
            .await
            .unwrap();
        let app = router().with_state(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/workspaces/docs/fs/../outside.txt")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
