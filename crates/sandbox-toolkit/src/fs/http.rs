//! Rejections from axum's own extractors, such as [`Path`] or [`Json`], still
//! return axum's plain text error body instead of the envelope; the extractors
//! defined here reject with [`AppError`].

use std::{
    collections::HashMap,
    convert::Infallible,
    path::{Component, Path, PathBuf},
};

use super::dir::{
    DirectoryError, ListRequest, create_directory, create_directory_recursive, read_directory,
    read_directory_recursive,
};
use super::file::{
    DownloadMode, FileContent, FileHeaders, ReadFileError, prepare_download, probe_file,
};
use super::glob::{self, GlobError, GlobRequest};
use super::meta::{MetadataError, read_metadata};
use super::model::{ResourceMetadata, ResourceOperation};
use crate::workspace::registry::TargetFile;
use crate::{AppError, AppState};
use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::{FromRequestParts, OriginalUri, Path as AxumPath, RawQuery, State};
use axum::http::Method;
use axum::http::StatusCode;
use axum::http::header::{self, HeaderValue};
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use axum::routing::{self, MethodRouter};
use tokio_stream::StreamExt;

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

fn resource_routes() -> MethodRouter<AppState> {
    routing::get(get_file)
        .head(head_file)
        .query(query_resource)
        .post(post_resource)
        .put(put_resource)
        .delete(delete_resource)
        .fallback(method_not_allowed)
}

struct ResourceTarget {
    resource: TargetFile,
    /// Empty addresses the workspace root.
    address: String,
}

impl ResourceTarget {
    /// The root itself is only addressable by the directory queries.
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

/// Parsing only decides whether the name is known at all, so an unknown name is
/// rejected before any method sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::EnumString)]
#[strum(serialize_all = "snake_case")]
enum ResourceType {
    Stream,
    Metadata,
    List,
    Glob,
    Realpath,
    Access,
    Lines,
    Directory,
    Symlink,
    Patch,
    Truncate,
    Copy,
    Move,
}

fn request_type(query: &RawQuery) -> Result<Option<ResourceType>, AppError> {
    query_value(query, "type")
        .map(|name| name.parse().map_err(|_| AppError::UnsupportedType(name)))
        .transpose()
}

fn download_mode(query: &RawQuery) -> Result<DownloadMode, AppError> {
    match request_type(query)? {
        None => Ok(DownloadMode::Direct),
        Some(ResourceType::Stream) => Ok(DownloadMode::Stream),
        Some(_) => Err(AppError::MethodNotAllowed(Method::GET)),
    }
}

fn reject_type(query: &RawQuery) -> Result<(), AppError> {
    match request_type(query)? {
        Some(_) => Err(AppError::MethodNotAllowed(Method::HEAD)),
        None => Ok(()),
    }
}

enum QueryOperation {
    Metadata,
    List,
    Glob,
    Realpath,
    Access,
    Lines,
}

impl TryFrom<ResourceType> for QueryOperation {
    type Error = AppError;

    fn try_from(value: ResourceType) -> Result<Self, Self::Error> {
        match value {
            ResourceType::Metadata => Ok(Self::Metadata),
            ResourceType::List => Ok(Self::List),
            ResourceType::Glob => Ok(Self::Glob),
            ResourceType::Realpath => Ok(Self::Realpath),
            ResourceType::Access => Ok(Self::Access),
            ResourceType::Lines => Ok(Self::Lines),
            _ => Err(AppError::MethodNotAllowed(Method::QUERY)),
        }
    }
}

fn query_operation(query: &RawQuery) -> Result<QueryOperation, AppError> {
    match request_type(query)? {
        Some(resource_type) => resource_type.try_into(),
        None => Err(AppError::UnsupportedType(
            "QUERY requires a `type`".to_owned(),
        )),
    }
}

fn list_recursive(query: &RawQuery) -> Result<bool, AppError> {
    match query_value(query, "depth").as_deref() {
        None => Ok(false),
        Some("infinity") => Ok(true),
        Some(value) => Err(AppError::BadRequest(format!("unsupported depth: {value}"))),
    }
}

fn query_value(query: &RawQuery, key: &str) -> Option<String> {
    let raw = query.0.as_deref()?;
    url::form_urlencoded::parse(raw.as_bytes())
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.into_owned())
}

fn query_values(query: &RawQuery, key: &str) -> Vec<String> {
    let Some(raw) = query.0.as_deref() else {
        return Vec::new();
    };
    url::form_urlencoded::parse(raw.as_bytes())
        .filter(|(name, _)| name == key)
        .map(|(_, value)| value.into_owned())
        .collect()
}

fn query_usize(query: &RawQuery, key: &str) -> Result<Option<usize>, AppError> {
    match query_value(query, key) {
        None => Ok(None),
        Some(value) => value
            .parse()
            .map(Some)
            .map_err(|_| AppError::BadRequest(format!("invalid {key}: {value}"))),
    }
}

/// A `limit` of zero is meaningless, so it is rejected for every windowed
/// endpoint rather than clamped to an empty page.
fn query_limit(query: &RawQuery) -> Result<Option<usize>, AppError> {
    let limit = query_usize(query, "limit")?;
    if limit == Some(0) {
        return Err(AppError::BadRequest("limit must be positive".to_owned()));
    }
    Ok(limit)
}

fn list_request(query: &RawQuery) -> Result<ListRequest, AppError> {
    Ok(ListRequest {
        offset: query_usize(query, "offset")?.unwrap_or(0),
        limit: query_limit(query)?,
    })
}

fn glob_request(query: &RawQuery) -> Result<GlobRequest, AppError> {
    let pattern = query_value(query, "pattern")
        .filter(|pattern| !pattern.is_empty())
        .ok_or_else(|| AppError::BadRequest("glob requires a `pattern`".to_owned()))?;

    Ok(GlobRequest {
        pattern,
        exclude: query_values(query, "exclude"),
        offset: query_usize(query, "offset")?.unwrap_or(0),
        limit: query_limit(query)?,
    })
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

#[derive(Debug, Default, serde::Deserialize)]
struct CreateDirectoryRequest {
    #[serde(default)]
    recursive: bool,
}

/// Upper bound on a control-plane JSON body, so an oversized or endless body is
/// rejected instead of buffered.
const MAX_REQUEST_BODY: usize = 64 * 1024;

async fn create_directory_request(body: Body) -> Result<CreateDirectoryRequest, AppError> {
    let bytes = axum::body::to_bytes(body, MAX_REQUEST_BODY)
        .await
        .map_err(|error| AppError::BadRequest(format!("failed to read request body: {error}")))?;

    if bytes.is_empty() {
        return Ok(CreateDirectoryRequest::default());
    }

    serde_json::from_slice(&bytes).map_err(|error| AppError::InvalidRequest(error.to_string()))
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

fn map_glob_error(error: GlobError) -> AppError {
    match error {
        GlobError::NotFound(path) => AppError::NotFound(path),
        GlobError::NotDirectory(path) => AppError::NotADirectory(path),
        GlobError::OutsideWorkspace(path) => {
            AppError::BadRequest(format!("path escapes workspace: {path}"))
        }
        GlobError::InvalidPattern(message) => AppError::BadRequest(message),
        GlobError::Io(error) => AppError::Internal(error.into()),
    }
}

fn map_directory_error(error: DirectoryError) -> AppError {
    match error {
        DirectoryError::NotFound(path) => AppError::NotFound(path),
        DirectoryError::NotDirectory(path) => AppError::NotADirectory(path),
        // A missing parent and an occupied target are both WebDAV `MKCOL`
        // conflicts, which is what `DirectoryError` exists to keep apart.
        DirectoryError::AlreadyExists(_) | DirectoryError::ParentNotFound(_) => {
            AppError::Conflict(error.to_string())
        }
        DirectoryError::OutsideWorkspace(path) => {
            AppError::BadRequest(format!("path escapes workspace: {path}"))
        }
        DirectoryError::Io(error) => AppError::Internal(error.into()),
    }
}

fn json_resource(metadata: &ResourceMetadata, status: StatusCode) -> Response {
    let mut response = Json(metadata).into_response();
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::ETAG,
        HeaderValue::from_str(&metadata.etag).expect("an entity-tag is a valid header value"),
    );
    response
}

impl FileHeaders {
    /// Inserting after the body is built matters: `IntoResponse` for bytes guesses
    /// `Content-Type` from the payload, and only a real `HeaderMap` insert reliably
    /// overrides that guess.
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

/// Values are kept verbatim: interpreting `ETag` lists and choosing the
/// precondition status is the handler's job.
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

/// The response shape depends on the `type` parameter, which is why this returns
/// [`Response`] rather than a concrete JSON type.
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

            Ok(json_resource(&metadata, StatusCode::OK))
        }
        QueryOperation::List => {
            ensure_empty_body(
                body,
                AppError::InvalidRequest("list takes no request body".to_owned()),
            )
            .await?;

            let request = list_request(&query)?;
            let directory = if list_recursive(&query)? {
                read_directory_recursive(&target.resource, &request).await
            } else {
                read_directory(&target.resource, &request).await
            }
            .map_err(map_directory_error)?;

            Ok(Json(&directory).into_response())
        }
        QueryOperation::Glob => {
            ensure_empty_body(
                body,
                AppError::InvalidRequest("glob takes no request body".to_owned()),
            )
            .await?;

            let directory = glob::search(&target.resource, &glob_request(&query)?)
                .await
                .map_err(map_glob_error)?;

            Ok(Json(&directory).into_response())
        }
        QueryOperation::Realpath => Err(AppError::NotImplemented("QUERY realpath")),
        QueryOperation::Access => Err(AppError::NotImplemented("QUERY access")),
        QueryOperation::Lines => Err(AppError::NotImplemented("QUERY lines")),
    }
}

async fn post_resource(
    target: ResourceTarget,
    _conditions: ConditionalHeaders,
    OriginalUri(_original_uri): OriginalUri,
    Json(_operation): Json<ResourceOperation>,
) -> Result<Response, AppError> {
    target.resource.require_writable()?;
    Err(AppError::NotImplemented("POST resource"))
}

async fn put_resource(
    State(_state): State<AppState>,
    target: ResourceTarget,
    query: RawQuery,
    _conditions: ConditionalHeaders,
    OriginalUri(_original_uri): OriginalUri,
    body: Body,
) -> Result<Response, AppError> {
    target.resource.require_writable()?;

    match request_type(&query)? {
        Some(ResourceType::Directory) => {
            target.require_resource_path()?;
            let request = create_directory_request(body).await?;

            let metadata = if request.recursive {
                create_directory_recursive(&target.resource).await
            } else {
                create_directory(&target.resource).await
            }
            .map_err(map_directory_error)?;

            Ok(json_resource(&metadata, StatusCode::CREATED))
        }
        Some(_) => Err(AppError::MethodNotAllowed(Method::PUT)),
        None => Err(AppError::NotImplemented("PUT resource")),
    }
}

async fn delete_resource(
    target: ResourceTarget,
    _conditions: ConditionalHeaders,
    OriginalUri(_original_uri): OriginalUri,
) -> Result<Response, AppError> {
    target.resource.require_writable()?;
    Err(AppError::NotImplemented("DELETE resource"))
}

/// axum answers unsupported methods with an empty body; the fallback keeps the
/// JSON error envelope. The `Allow` header is still added, because every method
/// this covers is absent from the [`MethodRouter`].
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
    use crate::workspace::model::{WorkspaceAccess, WorkspaceProperties};
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
            .register("docs", &dir.root(), WorkspaceProperties::default())
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
            .register("docs", &dir.root(), WorkspaceProperties::default())
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

        // `glob` belongs to `QUERY`, so it is routed to the handler; a missing
        // `pattern` is then the handler's own bad request rather than a routing
        // failure.
        let response = call(
            &app,
            Request::builder()
                .method("QUERY")
                .uri(format!("{uri}?type=glob"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
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
    async fn query_list_returns_direct_children() {
        let dir = TempDir::new();
        tokio::fs::write(dir.path().join("z.txt"), "hello")
            .await
            .unwrap();
        tokio::fs::create_dir(dir.path().join("a")).await.unwrap();
        tokio::fs::symlink("z.txt", dir.path().join("link"))
            .await
            .unwrap();
        let app = router().with_state(AppState::new("."));

        let response = call(
            &app,
            Request::builder()
                .method("QUERY")
                .uri(format!("/fs{}?type=list", dir.path().display()))
                .body(Body::empty())
                .unwrap(),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let entries = body["entries"].as_array().unwrap();
        let mut names: Vec<&str> = entries
            .iter()
            .map(|entry| entry["name"].as_str().unwrap())
            .collect();
        names.sort_unstable();
        assert_eq!(names, ["a", "link", "z.txt"]);
        let by_name = |name: &str| entries.iter().find(|entry| entry["name"] == name).unwrap();
        assert_eq!(by_name("a")["kind"], "directory");
        assert_eq!(by_name("link")["kind"], "symlink");
        assert_eq!(by_name("z.txt")["kind"], "file");
        assert_eq!(by_name("z.txt")["size"], 5);
    }

    #[tokio::test]
    async fn query_list_paginates() {
        let dir = TempDir::new();
        for name in ["a.txt", "b.txt", "c.txt"] {
            tokio::fs::write(dir.path().join(name), name).await.unwrap();
        }
        let app = router().with_state(AppState::new("."));
        let base = format!("/fs{}?type=list", dir.path().display());

        let response = call(
            &app,
            Request::builder()
                .method("QUERY")
                .uri(format!("{base}&limit=2"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["entries"].as_array().unwrap().len(), 2);
        assert_eq!(body["truncated"], true);

        let response = call(
            &app,
            Request::builder()
                .method("QUERY")
                .uri(format!("{base}&offset=3"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(body["entries"].as_array().unwrap().is_empty());
        assert_eq!(body["truncated"], false);
    }

    #[tokio::test]
    async fn query_list_recurses_with_depth_infinity() {
        let dir = TempDir::new();
        tokio::fs::create_dir_all(dir.path().join("a/b"))
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("a/b/c.txt"), "hello")
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("z.txt"), "z")
            .await
            .unwrap();
        let app = router().with_state(AppState::new("."));
        let base = format!("/fs{}?type=list", dir.path().display());

        let response = call(
            &app,
            Request::builder()
                .method("QUERY")
                .uri(&base)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let entries = body["entries"].as_array().unwrap();
        let mut names: Vec<&str> = entries
            .iter()
            .map(|entry| entry["name"].as_str().unwrap())
            .collect();
        names.sort_unstable();
        assert_eq!(names, ["a", "z.txt"]);

        let response = call(
            &app,
            Request::builder()
                .method("QUERY")
                .uri(format!("{base}&depth=infinity"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let entries = body["entries"].as_array().unwrap();
        let mut names: Vec<&str> = entries
            .iter()
            .map(|entry| entry["name"].as_str().unwrap())
            .collect();
        names.sort_unstable();
        assert_eq!(names, ["a", "b", "c.txt", "z.txt"]);
        let c_txt = entries
            .iter()
            .find(|entry| entry["name"] == "c.txt")
            .unwrap();
        assert_eq!(c_txt["kind"], "file");
        assert_eq!(c_txt["size"], 5);

        let response = call(
            &app,
            Request::builder()
                .method("QUERY")
                .uri(format!("{base}&depth=2"))
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

    #[tokio::test]
    async fn query_list_lists_the_workspace_root() {
        let dir = TempDir::new();
        tokio::fs::write(dir.path().join("note.txt"), "hello")
            .await
            .unwrap();
        let state = AppState::new(".");
        state
            .workspaces()
            .register("docs", &dir.root(), WorkspaceProperties::default())
            .await
            .unwrap();
        let app = router().with_state(state);

        let response = call(
            &app,
            Request::builder()
                .method("QUERY")
                .uri("/workspaces/docs/fs?type=list")
                .body(Body::empty())
                .unwrap(),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["entries"][0]["name"], "note.txt");
    }

    #[tokio::test]
    async fn query_list_rejects_a_file_and_a_missing_directory() {
        let dir = TempDir::new();
        tokio::fs::write(dir.path().join("note.txt"), "hello")
            .await
            .unwrap();
        let app = router().with_state(AppState::new("."));

        let response = call(
            &app,
            Request::builder()
                .method("QUERY")
                .uri(format!(
                    "/fs{}?type=list",
                    dir.path().join("note.txt").display()
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["error"]["code"].as_str(), Some("not_a_directory"));

        let response = call(
            &app,
            Request::builder()
                .method("QUERY")
                .uri(format!(
                    "/fs{}?type=list",
                    dir.path().join("missing").display()
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn query_glob_matches_a_pattern_in_a_workspace() {
        let dir = TempDir::new();
        tokio::fs::create_dir(dir.path().join("nested"))
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("a.txt"), "a")
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("b.md"), "b")
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("nested/c.txt"), "c")
            .await
            .unwrap();
        let state = AppState::new(".");
        state
            .workspaces()
            .register("docs", &dir.root(), WorkspaceProperties::default())
            .await
            .unwrap();
        let app = router().with_state(state);

        let response = call(
            &app,
            Request::builder()
                .method("QUERY")
                .uri("/workspaces/docs/fs?type=glob&pattern=**/*.txt")
                .body(Body::empty())
                .unwrap(),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let entries = body["entries"].as_array().unwrap();
        let mut names: Vec<&str> = entries
            .iter()
            .map(|entry| entry["name"].as_str().unwrap())
            .collect();
        names.sort_unstable();
        assert_eq!(names, ["a.txt", "c.txt"]);
        assert_eq!(body["truncated"], false);
    }

    #[tokio::test]
    async fn query_glob_excludes_and_truncates() {
        let dir = TempDir::new();
        tokio::fs::create_dir(dir.path().join("nested"))
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("a.txt"), "a")
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("b.md"), "b")
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("nested/c.txt"), "c")
            .await
            .unwrap();
        let app = router().with_state(AppState::new("."));

        let response = call(
            &app,
            Request::builder()
                .method("QUERY")
                .uri(format!(
                    "/fs{}?type=glob&pattern=**/*&exclude=nested&limit=1",
                    dir.path().display()
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
        let entries = body["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
        // Hits arrive in walk order, whose identity the filesystem does not fix.
        let name = entries[0]["name"].as_str().unwrap();
        assert!(["a.txt", "b.md"].contains(&name), "{name}");
        assert_eq!(body["truncated"], true);
    }

    #[tokio::test]
    async fn query_glob_rejects_a_file_and_a_missing_pattern() {
        let dir = TempDir::new();
        tokio::fs::write(dir.path().join("note.txt"), "hello")
            .await
            .unwrap();
        let app = router().with_state(AppState::new("."));

        for (uri, code) in [
            (
                format!(
                    "/fs{}?type=glob&pattern=*",
                    dir.path().join("note.txt").display()
                ),
                "not_a_directory",
            ),
            (
                format!("/fs{}?type=glob", dir.path().display()),
                "bad_request",
            ),
            (
                format!("/fs{}?type=glob&pattern=[", dir.path().display()),
                "bad_request",
            ),
        ] {
            let response = call(
                &app,
                Request::builder()
                    .method("QUERY")
                    .uri(&uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;

            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{uri}");
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(body["error"]["code"].as_str(), Some(code), "{uri}");
        }
    }

    #[tokio::test]
    async fn put_directory_creates_one_directory() {
        let dir = TempDir::new();
        let app = router().with_state(AppState::new("."));

        let response = call(
            &app,
            Request::builder()
                .method("PUT")
                .uri(format!(
                    "/fs{}?type=directory",
                    dir.path().join("child").display()
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await;

        assert_eq!(response.status(), StatusCode::CREATED);
        assert!(response.headers().contains_key(header::ETAG));
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["kind"], "directory");
        assert_eq!(body["name"], "child");
        assert!(dir.path().join("child").is_dir());
    }

    #[tokio::test]
    async fn put_directory_reports_conflicts_and_unknown_types() {
        let dir = TempDir::new();
        tokio::fs::create_dir(dir.path().join("existing"))
            .await
            .unwrap();
        let app = router().with_state(AppState::new("."));

        let response = call(
            &app,
            Request::builder()
                .method("PUT")
                .uri(format!(
                    "/fs{}?type=directory",
                    dir.path().join("existing").display()
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);

        let response = call(
            &app,
            Request::builder()
                .method("PUT")
                .uri(format!(
                    "/fs{}?type=directory",
                    dir.path().join("missing/child").display()
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);

        let response = call(
            &app,
            Request::builder()
                .method("PUT")
                .uri(format!(
                    "/fs{}?type=metadata",
                    dir.path().join("child").display()
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn put_directory_recursive_creates_missing_parents() {
        let dir = TempDir::new();
        let app = router().with_state(AppState::new("."));

        let response = call(
            &app,
            Request::builder()
                .method("PUT")
                .uri(format!(
                    "/fs{}?type=directory",
                    dir.path().join("a/b/c").display()
                ))
                .body(Body::from(r#"{"recursive":true}"#))
                .unwrap(),
        )
        .await;

        assert_eq!(response.status(), StatusCode::CREATED);
        assert!(dir.path().join("a/b/c").is_dir());
    }

    #[tokio::test]
    async fn put_directory_rejects_a_malformed_body() {
        let dir = TempDir::new();
        let app = router().with_state(AppState::new("."));

        let response = call(
            &app,
            Request::builder()
                .method("PUT")
                .uri(format!(
                    "/fs{}?type=directory",
                    dir.path().join("child").display()
                ))
                .body(Body::from(r#"{"recursive":"yes"}"#))
                .unwrap(),
        )
        .await;

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["error"]["code"], "invalid_request");
        assert!(!dir.path().join("child").exists());
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
            .register("docs", &dir.root(), WorkspaceProperties::default())
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
            .register("docs", &dir.root(), WorkspaceProperties::default())
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

    #[tokio::test]
    async fn rejects_mutations_against_a_read_only_workspace() {
        let dir = TempDir::new();
        tokio::fs::write(dir.path().join("note.txt"), "hello")
            .await
            .unwrap();
        let state = AppState::new(".");
        state
            .workspaces()
            .register(
                "docs",
                &dir.root(),
                WorkspaceProperties {
                    access: WorkspaceAccess::ReadOnly,
                },
            )
            .await
            .unwrap();
        let app = router().with_state(state);

        // Reads, directory queries and metadata are unaffected.
        let response = call(
            &app,
            Request::builder()
                .method("QUERY")
                .uri("/workspaces/docs/fs?type=list")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        // Creating a directory is rejected.
        let response = call(
            &app,
            Request::builder()
                .method("PUT")
                .uri("/workspaces/docs/fs/child?type=directory")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["error"]["code"], "read_only_workspace");
        assert!(!dir.path().join("child").exists());

        // Deleting is rejected too, before the operation's own implementation.
        let response = call(
            &app,
            Request::builder()
                .method("DELETE")
                .uri("/workspaces/docs/fs/note.txt")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(dir.path().join("note.txt").exists());
    }
}
