//! File routes of a workspace.
//!
//! Operations that WebDAV expresses as extensions to the method itself are
//! submitted as a `POST` body instead ([`FileOperation`]), because axum routes
//! only standard HTTP methods. Keeping one [`MethodRouter`] per file URI then
//! lets axum supply `405 Method Not Allowed` with its `Allow` header.
//!
//! One deviation remains: rejections raised by axum's own extractors, such as
//! [`Path`] or [`Json`], still return axum's plain text error body instead of the
//! envelope. The extractors defined in this module reject with [`AppError`].

use std::{
    collections::HashMap,
    convert::Infallible,
    path::{Component, Path, PathBuf},
};

use super::model::{FileOperation, FilePath, ReadFileRequest};
use crate::workspace::registry::TargetFile;
use crate::{AppError, AppState};
use axum::Json;
use axum::body::Body;
use axum::extract::{FromRequestParts, OriginalUri, Path as AxumPath, Query, State};
use axum::http::Method;
use axum::http::header::{self, HeaderName, HeaderValue};
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use axum::routing::{self, MethodRouter};

use super::read::{ReadFileError, read_file};

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

#[derive(Debug, Default, serde::Deserialize)]
struct ReadQuery {
    offset: Option<u64>,
    limit: Option<u64>,
}

struct ReadRange(ReadFileRequest);

impl FromRequestParts<AppState> for ReadRange {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let Query(query) = Query::<ReadQuery>::from_request_parts(parts, _state)
            .await
            .map_err(|_| AppError::BadRequest("offset and limit must be integers".to_owned()))?;
        Ok(Self(ReadFileRequest {
            offset: query
                .offset
                .unwrap_or(0)
                .try_into()
                .map_err(|_| AppError::BadRequest("offset is too large".to_owned()))?,
            limit: query
                .limit
                .map(|value| {
                    value
                        .try_into()
                        .map_err(|_| AppError::BadRequest("limit is too large".to_owned()))
                })
                .transpose()?,
        }))
    }
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
        ReadFileError::InvalidFile(message) => AppError::BadRequest(message),
        ReadFileError::Io(error) => AppError::Internal(error.into()),
    }
}

/// Route table shared by the workspace root and every file below it.
///
pub(super) fn file_routes() -> MethodRouter<AppState> {
    routing::get(get_file)
        .post(post_file)
        .put(put_file)
        .delete(delete_file)
        .fallback(method_not_allowed)
}

/// The `Depth` request header (RFC 4918), absent from [`axum::http::header`],
/// which models only non-WebDAV headers.
const DEPTH: HeaderName = HeaderName::from_static("depth");

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum FileDepth {
    /// `Depth: 0`: the target file.
    #[default]
    Zero,
    /// `Depth: 1`: the target directory and its direct children.
    One,
}

impl<S> FromRequestParts<S> for FileDepth
where
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let Some(value) = parts.headers.get(&DEPTH) else {
            return Ok(Self::default());
        };

        match value.as_bytes() {
            b"0" => Ok(Self::Zero),
            b"1" => Ok(Self::One),
            _ => Err(AppError::BadRequest("Depth must be 0 or 1".to_owned())),
        }
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
    range: ReadRange,
    _depth: FileDepth,
    OriginalUri(_original_uri): OriginalUri,
) -> Result<Response, AppError> {
    let body = read_file(target.0, range.0).await.map_err(map_read_error)?;

    Ok(([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], body).into_response())
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

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use crate::AppState;
    use crate::workspace::registry::test_support::TempDir;

    use super::super::router;

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
