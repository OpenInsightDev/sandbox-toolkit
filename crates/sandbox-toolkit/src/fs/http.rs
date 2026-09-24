//! The resource API endpoints.
//!
//! Resources map directly to URIs under the workspace root, and every resource URI shares
//! one [`MethodRouter`], so axum supplies `405 Method Not Allowed` with its `Allow`
//! header. Every operation is then a method plus a `type` query parameter: the
//! control plane reads through `QUERY`, and the mutations WebDAV spells as
//! extension methods become `PUT`, `PATCH` and `POST` with their own `type`
//! values.
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
    DownloadMode, FileContent, FileHeaders, ReadFileError, prepare_download, probe_file,
};
use super::meta::{MetadataError, read_metadata};
use super::model::{ResourceOperation, ResourcePath};
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

/// Build the resource router.
///
/// Paths include the owning workspace because a resource is always addressed
/// within one, but the routes stand on their own rather than being nested into
/// the workspace control plane.
pub(crate) fn router() -> Router<AppState> {
    // A wildcard capture never matches an empty path, and `/fs` and `/fs/`
    // are distinct routes in axum, so addressing the workspace root needs these
    // two routes; the wildcard route matches neither of them.
    Router::new()
        .route("/workspaces/{workspace_id}/fs", resource_routes())
        .route("/workspaces/{workspace_id}/fs/", resource_routes())
        .route("/workspaces/{workspace_id}/fs/{*path}", resource_routes())
        .route("/fs", resource_routes())
        .route("/fs/", resource_routes())
        .route("/fs/{*path}", resource_routes())
}

/// Route table shared by the workspace root and every resource below it.
fn resource_routes() -> MethodRouter<AppState> {
    routing::get(get_file)
        .head(head_file)
        .query(query_resource)
        .post(post_resource)
        .put(put_resource)
        .delete(delete_resource)
        .fallback(method_not_allowed)
}

/// The resource an fs URI addresses, with the path as its addressing mode
/// spells it.
struct ResourceTarget {
    resource: TargetFile,
    /// Workspace-relative path, or the remote absolute path with its leading
    /// slash. Empty addresses the workspace root.
    address: String,
}

impl ResourceTarget {
    /// A resource operation addresses a resource below the root; the root itself is
    /// only addressable by the directory queries.
    fn require_resource_path(&self) -> Result<(), AppError> {
        if self.address.is_empty() {
            return Err(AppError::BadRequest(
                "resource path must not be empty".to_owned(),
            ));
        }
        Ok(())
    }
}

impl FromRequestParts<AppState> for ResourceTarget {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let AxumPath(captures): AxumPath<HashMap<String, String>> =
            AxumPath::from_request_parts(parts, state)
                .await
                .map_err(|_| AppError::BadRequest("invalid resource path".to_owned()))?;
        let raw = captures.get("path").map(String::as_str).unwrap_or("");
        let path = validate_path(raw)?;

        Ok(if let Some(workspace_id) = captures.get("workspace_id") {
            Self {
                resource: state.workspaces().target_file(workspace_id, path)?,
                address: raw.to_owned(),
            }
        } else {
            Self {
                resource: TargetFile::Absolute(PathBuf::from("/").join(path)),
                address: format!("/{raw}"),
            }
        })
    }
}

/// Every `type` value the resource API accepts.
///
/// A name outside this list is `422 unsupported_type`; what a listed name means
/// for the request's method is that method's business to decide.
const KNOWN_TYPES: &[&str] = &[
    "stream",
    "metadata",
    "list",
    "glob",
    "realpath",
    "access",
    "lines",
    "directory",
    "symlink",
    "patch",
    "truncate",
    "copy",
    "move",
];

/// The `type` parameter, checked against [`KNOWN_TYPES`].
fn request_type(query: &RawQuery) -> Result<Option<String>, AppError> {
    let Some(name) = query_value(query, "type") else {
        return Ok(None);
    };
    if KNOWN_TYPES.contains(&name.as_str()) {
        Ok(Some(name))
    } else {
        Err(AppError::UnsupportedType(name))
    }
}

/// Resolve the response shape of a `GET`.
///
/// `stream` streams the raw bytes; without a `type` the whole content is sent as
/// one body. Any other `type` names a control-plane operation, so the
/// combination is `405 method_not_allowed`.
fn download_mode(query: &RawQuery) -> Result<DownloadMode, AppError> {
    match request_type(query)?.as_deref() {
        None => Ok(DownloadMode::Direct),
        Some("stream") => Ok(DownloadMode::Stream),
        Some(_) => Err(AppError::MethodNotAllowed(Method::GET)),
    }
}

/// Reject a `type` on `HEAD`, whose response shape is fixed.
fn reject_type(query: &RawQuery) -> Result<(), AppError> {
    match request_type(query)? {
        Some(_) => Err(AppError::MethodNotAllowed(Method::HEAD)),
        None => Ok(()),
    }
}

/// The control-plane read a `QUERY` asks for.
enum QueryOperation {
    Metadata,
    List,
    Glob,
    Realpath,
    Access,
    Lines,
}

/// Resolve the `type` of a `QUERY`, which is required.
fn query_operation(query: &RawQuery) -> Result<QueryOperation, AppError> {
    let Some(name) = request_type(query)? else {
        return Err(AppError::UnsupportedType(
            "QUERY requires a `type`".to_owned(),
        ));
    };

    match name.as_str() {
        "metadata" => Ok(QueryOperation::Metadata),
        "list" => Ok(QueryOperation::List),
        "glob" => Ok(QueryOperation::Glob),
        "realpath" => Ok(QueryOperation::Realpath),
        "access" => Ok(QueryOperation::Access),
        "lines" => Ok(QueryOperation::Lines),
        _ => Err(AppError::MethodNotAllowed(Method::QUERY)),
    }
}

/// The first value of a query parameter, percent decoded once.
fn query_value(query: &RawQuery, key: &str) -> Option<String> {
    let raw = query.0.as_deref()?;
    url::form_urlencoded::parse(raw.as_bytes())
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.into_owned())
}

/// A read request must carry no body, so any data frame rejects it with the
/// failure its caller reports.
async fn ensure_empty_body(body: Body, rejection: AppError) -> Result<(), AppError> {
    let mut stream = body.into_data_stream();
    while let Some(frame) = stream.next().await {
        let frame = frame.map_err(|error| {
            AppError::BadRequest(format!("failed to read request body: {error}"))
        })?;
        if !frame.is_empty() {
            return Err(rejection);
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

fn map_metadata_error(error: MetadataError) -> AppError {
    match error {
        MetadataError::NotFound(path) => AppError::NotFound(path),
        MetadataError::OutsideWorkspace(path) => {
            AppError::BadRequest(format!("path escapes workspace: {path}"))
        }
        MetadataError::Io(error) => AppError::Internal(error.into()),
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

/// Conditional request headers shared by resource mutations.
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

/// The response shape depends on the `type` parameter, which is why this handler
/// returns [`Response`] rather than a concrete JSON type.
async fn get_file(
    State(_state): State<AppState>,
    target: ResourceTarget,
    query: RawQuery,
    OriginalUri(_original_uri): OriginalUri,
    body: Body,
) -> Result<Response, AppError> {
    let mode = download_mode(&query)?;
    target.require_resource_path()?;
    ensure_empty_body(
        body,
        AppError::BadRequest("request body must be empty".to_owned()),
    )
    .await?;

    let prepared = prepare_download(&target.resource, mode)
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
    target: ResourceTarget,
    query: RawQuery,
    OriginalUri(_original_uri): OriginalUri,
    body: Body,
) -> Result<Response, AppError> {
    reject_type(&query)?;
    target.require_resource_path()?;
    ensure_empty_body(
        body,
        AppError::BadRequest("request body must be empty".to_owned()),
    )
    .await?;

    let headers = probe_file(&target.resource).await.map_err(map_read_error)?;

    let mut response = Body::empty().into_response();
    headers.stamp_on(&mut response);

    Ok(response)
}

/// Answer the control-plane read named by `type`.
async fn query_resource(
    State(_state): State<AppState>,
    target: ResourceTarget,
    query: RawQuery,
    body: Body,
) -> Result<Response, AppError> {
    match query_operation(&query)? {
        QueryOperation::Metadata => {
            ensure_empty_body(
                body,
                AppError::InvalidRequest("metadata takes no request body".to_owned()),
            )
            .await?;

            let metadata = read_metadata(&target.resource)
                .await
                .map_err(map_metadata_error)?;

            let mut response = Json(&metadata).into_response();
            response.headers_mut().insert(
                header::ETAG,
                HeaderValue::from_str(&metadata.etag)
                    .expect("an entity-tag is a valid header value"),
            );
            Ok(response)
        }
        QueryOperation::List => Err(AppError::NotImplemented("QUERY list")),
        QueryOperation::Glob => Err(AppError::NotImplemented("QUERY glob")),
        QueryOperation::Realpath => Err(AppError::NotImplemented("QUERY realpath")),
        QueryOperation::Access => Err(AppError::NotImplemented("QUERY access")),
        QueryOperation::Lines => Err(AppError::NotImplemented("QUERY lines")),
    }
}

async fn post_resource(
    State(_state): State<AppState>,
    AxumPath(_resource): AxumPath<ResourcePath>,
    _conditions: ConditionalHeaders,
    OriginalUri(_original_uri): OriginalUri,
    Json(_operation): Json<ResourceOperation>,
) -> Result<Response, AppError> {
    Err(AppError::NotImplemented("POST resource"))
}

async fn put_resource(
    State(_state): State<AppState>,
    AxumPath(_resource): AxumPath<ResourcePath>,
    _conditions: ConditionalHeaders,
    OriginalUri(_original_uri): OriginalUri,
    _body: Body,
) -> Result<Response, AppError> {
    Err(AppError::NotImplemented("PUT resource"))
}

async fn delete_resource(
    State(_state): State<AppState>,
    AxumPath(_resource): AxumPath<ResourcePath>,
    _conditions: ConditionalHeaders,
    OriginalUri(_original_uri): OriginalUri,
) -> Result<Response, AppError> {
    Err(AppError::NotImplemented("DELETE resource"))
}

/// Fallback for the methods a resource URI does not support.
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
    async fn get_streams_a_file_when_type_is_stream() {
        let dir = TempDir::new();
        tokio::fs::write(dir.path().join("note.txt"), "hello")
            .await
            .unwrap();
        let uri = format!("/fs{}?type=stream", dir.path().join("note.txt").display());
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

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"hello");
    }

    #[tokio::test]
    async fn get_rejects_a_mismatched_type_a_body_and_a_directory() {
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
                .method("HEAD")
                .uri(format!("{file_uri}?type=stream"))
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
    async fn query_metadata_describes_a_file() {
        let dir = TempDir::new();
        tokio::fs::write(dir.path().join("note.txt"), "hello")
            .await
            .unwrap();
        let uri = format!("/fs{}?type=metadata", dir.path().join("note.txt").display());
        let app = router().with_state(AppState::new("."));

        let response = call(
            &app,
            Request::builder()
                .method("QUERY")
                .uri(&uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        let etag = response.headers().get(header::ETAG).unwrap().clone();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["name"], "note.txt");
        assert_eq!(
            body["path"],
            dir.path().join("note.txt").display().to_string()
        );
        assert_eq!(body["kind"], "file");
        assert_eq!(body["size"], 5);
        assert_eq!(body["etag"].as_str(), etag.to_str().ok());
        assert!(body["target"].is_null());
        chrono::DateTime::parse_from_rfc3339(body["modified_at"].as_str().unwrap()).unwrap();
    }

    #[tokio::test]
    async fn query_metadata_describes_directories_and_the_workspace_root() {
        let dir = TempDir::new();
        tokio::fs::create_dir(dir.path().join("notes"))
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
                .method("QUERY")
                .uri(format!(
                    "/fs{}?type=metadata",
                    dir.path().join("notes").display()
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["kind"], "directory");
        assert_eq!(body["size"], 0);

        let response = call(
            &app,
            Request::builder()
                .method("QUERY")
                .uri("/workspaces/docs/fs/notes?type=metadata")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["path"], "notes");

        let response = call(
            &app,
            Request::builder()
                .method("QUERY")
                .uri("/workspaces/docs/fs?type=metadata")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["path"], "");
        assert_eq!(body["kind"], "directory");
    }

    #[tokio::test]
    async fn query_metadata_describes_a_symlink_without_following_it() {
        let dir = TempDir::new();
        tokio::fs::write(dir.path().join("note.txt"), "hello")
            .await
            .unwrap();
        tokio::fs::symlink("note.txt", dir.path().join("link"))
            .await
            .unwrap();
        tokio::fs::symlink("missing.txt", dir.path().join("ghost"))
            .await
            .unwrap();
        let app = router().with_state(AppState::new("."));

        for (name, target) in [("link", "note.txt"), ("ghost", "missing.txt")] {
            let response = call(
                &app,
                Request::builder()
                    .method("QUERY")
                    .uri(format!(
                        "/fs{}?type=metadata",
                        dir.path().join(name).display()
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;

            assert_eq!(response.status(), StatusCode::OK);
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(body["name"], name);
            assert_eq!(body["kind"], "symlink");
            assert_eq!(body["size"], 0);
            assert_eq!(body["target"], target);
        }
    }

    #[tokio::test]
    async fn query_metadata_confines_workspace_paths() {
        let dir = TempDir::new();
        tokio::fs::symlink("..", dir.path().join("escape"))
            .await
            .unwrap();
        tokio::fs::symlink("../nope", dir.path().join("dangling-escape"))
            .await
            .unwrap();
        let state = AppState::new(".");
        state
            .workspaces()
            .register("docs", &dir.root())
            .await
            .unwrap();
        let app = router().with_state(state);

        for path in ["escape", "dangling-escape"] {
            let response = call(
                &app,
                Request::builder()
                    .method("QUERY")
                    .uri(format!("/workspaces/docs/fs/{path}?type=metadata"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;

            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(body["error"]["code"].as_str(), Some("bad_request"));
        }
    }

    #[tokio::test]
    async fn query_requires_a_type_that_belongs_to_the_query_method() {
        let dir = TempDir::new();
        tokio::fs::write(dir.path().join("note.txt"), "hello")
            .await
            .unwrap();
        let uri = format!("/fs{}", dir.path().join("note.txt").display());
        let app = router().with_state(AppState::new("."));

        for (query, status, code) in [
            (
                "",
                StatusCode::UNPROCESSABLE_ENTITY,
                Some("unsupported_type"),
            ),
            (
                "?type=stream",
                StatusCode::METHOD_NOT_ALLOWED,
                Some("method_not_allowed"),
            ),
            (
                "?type=bogus",
                StatusCode::UNPROCESSABLE_ENTITY,
                Some("unsupported_type"),
            ),
        ] {
            let response = call(
                &app,
                Request::builder()
                    .method("QUERY")
                    .uri(format!("{uri}{query}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;

            assert_eq!(response.status(), status);
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(body["error"]["code"].as_str(), code);
        }

        let response = call(
            &app,
            Request::builder()
                .method("QUERY")
                .uri(format!("{uri}?type=list"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    }

    #[tokio::test]
    async fn query_metadata_rejects_a_body_and_reports_missing_resources() {
        let dir = TempDir::new();
        let app = router().with_state(AppState::new("."));

        let response = call(
            &app,
            Request::builder()
                .method("QUERY")
                .uri(format!("/fs{}?type=metadata", dir.path().display()))
                .header(header::CONTENT_LENGTH, "2")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["error"]["code"].as_str(), Some("invalid_request"));

        let response = call(
            &app,
            Request::builder()
                .method("QUERY")
                .uri(format!(
                    "/fs{}?type=metadata",
                    dir.path().join("missing.txt").display()
                ))
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
