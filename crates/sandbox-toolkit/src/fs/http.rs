//! The method selects the operation class and `type` the exact operation, so
//! `QUERY`, `PUT`, `PATCH` and `POST` each dispatch on it while `DELETE` has none.
//! `QUERY ?type=content` is wired to its implementation; every other handler is
//! a stub.

use std::collections::HashMap;

use axum::body::Body;
use axum::extract::{FromRequestParts, Path, Query};
use axum::http::header;
use axum::http::request::Parts;
use axum::http::{HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{self, MethodRouter};
use axum::{Json, Router};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use tokio_util::io::ReaderStream;

use super::file::{self, FileError};
use super::model::{
    AccessRequest, ContentRequest, CreateSymlinkRequest, DeleteRequest, LinesRequest,
    MetadataRequest, PatchMetadataRequest, RealpathRequest, StreamRequest, TruncateRequest,
    WriteFileRequest,
};
use crate::path::PathError;
use crate::workspace::registry::Workspace;
use crate::{AppError, AppState};

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/fs", resource_routes())
        .route("/workspaces/{workspace_id}/fs", resource_routes())
}

fn resource_routes() -> MethodRouter<AppState> {
    routing::query(query_endpoint)
        .put(put_endpoint)
        .patch(patch_endpoint)
        .post(post_endpoint)
        .delete(delete_endpoint)
        .fallback(method_not_allowed)
}

/// Workspace mode carries the registered workspace, which fixes the boundary and
/// the write permission; direct mode has neither.
struct FsTarget {
    workspace: Option<Workspace>,
}

impl FsTarget {
    fn require_writable(&self) -> Result<(), AppError> {
        if let Some(workspace) = &self.workspace {
            workspace.require_writable()?;
        }
        Ok(())
    }
}

impl FromRequestParts<AppState> for FsTarget {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let Path(captures): Path<HashMap<String, String>> = Path::from_request_parts(parts, state)
            .await
            .map_err(|_| AppError::BadRequest("invalid filesystem path".to_owned()))?;

        match captures.get("workspace_id") {
            Some(id) => Ok(Self {
                workspace: Some(state.resolve_workspace(id).await?),
            }),
            None => Ok(Self { workspace: None }),
        }
    }
}

/// `type` names are scoped to the method that uses them, so each method has its
/// own set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::EnumString)]
#[strum(serialize_all = "snake_case")]
enum QueryType {
    Content,
    Stream,
    Metadata,
    List,
    Glob,
    Realpath,
    Access,
    Lines,
    Watch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::EnumString)]
#[strum(serialize_all = "snake_case")]
enum PutType {
    File,
    Sink,
    Directory,
    Symlink,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::EnumString)]
#[strum(serialize_all = "snake_case")]
enum PatchType {
    Metadata,
    Patch,
    Truncate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::EnumString)]
#[strum(serialize_all = "snake_case")]
enum PostType {
    Copy,
    Move,
}

#[derive(Debug, Default, Deserialize)]
struct ResourceQuery {
    #[serde(rename = "type")]
    kind: Option<String>,
}

/// A name belonging to another method is `405`, an unknown name `422`, and an
/// absent one `422`.
fn parse_type<T: std::str::FromStr>(query: &ResourceQuery, method: &Method) -> Result<T, AppError> {
    let name = query
        .kind
        .as_deref()
        .ok_or_else(|| AppError::UnsupportedType(format!("{method} requires a `type`")))?;

    name.parse().map_err(|_| {
        if is_known_type(name) {
            AppError::MethodNotAllowed(method.clone())
        } else {
            AppError::UnsupportedType(name.to_owned())
        }
    })
}

/// Every name any method defines. Reached only after the current method's own
/// parse fails, so a match here is necessarily another method's type.
fn is_known_type(name: &str) -> bool {
    name.parse::<QueryType>().is_ok()
        || name.parse::<PutType>().is_ok()
        || name.parse::<PatchType>().is_ok()
        || name.parse::<PostType>().is_ok()
}

async fn query_endpoint(
    target: FsTarget,
    Query(query): Query<ResourceQuery>,
    body: Body,
) -> Result<Response, AppError> {
    match parse_type::<QueryType>(&query, &Method::QUERY)? {
        QueryType::Content => content(target, body).await,
        QueryType::Stream => stream(target, body).await,
        QueryType::Metadata => metadata(target, body).await,
        QueryType::List => Err(not_implemented("QUERY ?type=list")),
        QueryType::Glob => Err(not_implemented("QUERY ?type=glob")),
        QueryType::Realpath => realpath(target, body).await,
        QueryType::Access => access(target, body).await,
        QueryType::Lines => lines(target, body).await,
        QueryType::Watch => Err(not_implemented("QUERY ?type=watch")),
    }
}

async fn content(target: FsTarget, body: Body) -> Result<Response, AppError> {
    let request: ContentRequest = read_json(body).await?;
    let content = file::read(target.workspace.as_ref(), request).await?;
    let etag = content.etag.clone();

    let mut response = Json(content).into_response();
    insert_etag(&mut response, &etag);

    Ok(response)
}

async fn stream(target: FsTarget, body: Body) -> Result<Response, AppError> {
    let request: StreamRequest = read_json(body).await?;
    let streamed = file::stream(target.workspace.as_ref(), request).await?;

    let mut response = Response::new(Body::from_stream(ReaderStream::new(streamed.file)));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&streamed.content_type).expect("a mime type is a valid header value"),
    );
    headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&streamed.size.to_string())
            .expect("a length is a valid header value"),
    );
    headers.insert(
        header::LAST_MODIFIED,
        HeaderValue::from_str(&httpdate::fmt_http_date(streamed.modified))
            .expect("an HTTP date is a valid header value"),
    );
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("none"));
    insert_etag(&mut response, &streamed.etag);

    Ok(response)
}

async fn metadata(target: FsTarget, body: Body) -> Result<Response, AppError> {
    let request: MetadataRequest = read_json(body).await?;
    let metadata = file::metadata(target.workspace.as_ref(), request).await?;
    let etag = metadata.etag.clone();

    let mut response = Json(metadata).into_response();
    insert_etag(&mut response, &etag);

    Ok(response)
}

async fn realpath(target: FsTarget, body: Body) -> Result<Response, AppError> {
    let request: RealpathRequest = read_json(body).await?;
    let response = file::realpath(target.workspace.as_ref(), request).await?;

    Ok(Json(response).into_response())
}

async fn access(target: FsTarget, body: Body) -> Result<Response, AppError> {
    let request: AccessRequest = read_json(body).await?;
    let response = file::access(target.workspace.as_ref(), request).await?;

    Ok(Json(response).into_response())
}

async fn lines(target: FsTarget, body: Body) -> Result<Response, AppError> {
    let request: LinesRequest = read_json(body).await?;
    let response = file::lines(target.workspace.as_ref(), request).await?;

    Ok(Json(response).into_response())
}

fn insert_etag(response: &mut Response, etag: &str) {
    response.headers_mut().insert(
        header::ETAG,
        HeaderValue::from_str(etag).expect("an entity-tag is a valid header value"),
    );
}

/// An upper bound on a control-plane JSON body, so an oversized or endless body
/// is rejected instead of buffered.
const MAX_REQUEST_BODY: usize = 64 * 1024;

/// Reading the body by hand rather than through [`Json`] keeps a malformed body
/// on the API's own `422 invalid_request` envelope.
async fn read_json<T: DeserializeOwned>(body: Body) -> Result<T, AppError> {
    let bytes = axum::body::to_bytes(body, MAX_REQUEST_BODY)
        .await
        .map_err(|error| AppError::BadRequest(format!("failed to read request body: {error}")))?;

    serde_json::from_slice(&bytes).map_err(|error| AppError::InvalidRequest(error.to_string()))
}

async fn put_endpoint(
    target: FsTarget,
    Query(query): Query<ResourceQuery>,
    body: Body,
) -> Result<Response, AppError> {
    target.require_writable()?;

    match parse_type::<PutType>(&query, &Method::PUT)? {
        PutType::File => {
            let request: WriteFileRequest = read_json(body).await?;
            let outcome = file::write_file(target.workspace.as_ref(), request).await?;
            let status = if outcome.created {
                StatusCode::CREATED
            } else {
                StatusCode::OK
            };

            Ok(mutated(status, outcome.metadata))
        }
        PutType::Symlink => {
            let request: CreateSymlinkRequest = read_json(body).await?;
            let metadata = file::create_symlink(target.workspace.as_ref(), request).await?;

            Ok(mutated(StatusCode::CREATED, metadata))
        }
        PutType::Sink => Err(not_implemented("PUT ?type=sink")),
        PutType::Directory => Err(not_implemented("PUT ?type=directory")),
    }
}

async fn patch_endpoint(
    target: FsTarget,
    Query(query): Query<ResourceQuery>,
    body: Body,
) -> Result<Response, AppError> {
    target.require_writable()?;

    match parse_type::<PatchType>(&query, &Method::PATCH)? {
        PatchType::Metadata => {
            let request: PatchMetadataRequest = read_json(body).await?;
            let metadata = file::patch_metadata(target.workspace.as_ref(), request).await?;

            Ok(mutated(StatusCode::OK, metadata))
        }
        PatchType::Truncate => {
            let request: TruncateRequest = read_json(body).await?;
            let metadata = file::truncate(target.workspace.as_ref(), request).await?;

            Ok(mutated(StatusCode::OK, metadata))
        }
        PatchType::Patch => Err(not_implemented("PATCH ?type=patch")),
    }
}

async fn post_endpoint(
    target: FsTarget,
    Query(query): Query<ResourceQuery>,
) -> Result<Response, AppError> {
    target.require_writable()?;

    match parse_type::<PostType>(&query, &Method::POST)? {
        PostType::Copy => Err(not_implemented("POST ?type=copy")),
        PostType::Move => Err(not_implemented("POST ?type=move")),
    }
}

async fn delete_endpoint(target: FsTarget, body: Body) -> Result<Response, AppError> {
    target.require_writable()?;

    let request: DeleteRequest = read_json(body).await?;
    file::delete(target.workspace.as_ref(), request).await?;

    Ok(StatusCode::NO_CONTENT.into_response())
}

/// A mutation answers with the resulting `ResourceMetadata` and its new ETag.
fn mutated(status: StatusCode, metadata: super::model::ResourceMetadata) -> Response {
    let etag = metadata.etag.clone();
    let mut response = (status, Json(metadata)).into_response();
    insert_etag(&mut response, &etag);

    response
}

fn not_implemented(operation: &'static str) -> AppError {
    AppError::NotImplemented(operation)
}

impl From<PathError> for AppError {
    fn from(error: PathError) -> Self {
        let message = error.to_string();

        match error {
            PathError::NotFound(path) => Self::NotFound(path),
            PathError::InvalidPath { .. } => Self::BadRequest(message),
            PathError::Io(source) => Self::Internal(source.into()),
        }
    }
}

impl From<FileError> for AppError {
    fn from(error: FileError) -> Self {
        let message = error.to_string();

        match error {
            FileError::NotAFile(path) => Self::NotAFile(path),
            FileError::NotUtf8 | FileError::InvalidRequest(_) => Self::InvalidRequest(message),
            FileError::BadRequest(_) => Self::BadRequest(message),
            FileError::AlreadyExists(_) => Self::PreconditionFailed(message),
            FileError::Path(error) => error.into(),
            // A missing entry surfaces as an I/O error from the metadata or open
            // call, which is the API's `not_found`.
            FileError::Io(source) if source.kind() == std::io::ErrorKind::NotFound => {
                Self::NotFound(message)
            }
            FileError::Io(source) => Self::Internal(source.into()),
        }
    }
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
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use super::*;
    use crate::AppState;
    use crate::workspace::model::{WorkspaceAccess, WorkspaceProperties};
    use crate::workspace::registry::test_support::TempDir;

    fn app() -> Router {
        router().with_state(AppState::new("."))
    }

    async fn call(app: &Router, method: &str, uri: &str) -> StatusCode {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::empty())
            .unwrap();

        app.clone().oneshot(request).await.unwrap().status()
    }

    async fn send(app: &Router, request: Request<Body>) -> Response {
        app.clone().oneshot(request).await.unwrap()
    }

    fn content_request(base: &str, body: serde_json::Value) -> Request<Body> {
        list_request(base, "content", body)
    }

    fn list_request(base: &str, kind: &str, body: serde_json::Value) -> Request<Body> {
        Request::builder()
            .method("QUERY")
            .uri(format!("{base}?type={kind}"))
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    fn mutation_request(
        method: &str,
        base: &str,
        kind: Option<&str>,
        body: serde_json::Value,
    ) -> Request<Body> {
        let uri = match kind {
            Some(kind) => format!("{base}?type={kind}"),
            None => base.to_owned(),
        };

        Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    async fn json_body(response: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();

        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn content_returns_a_file_as_utf8_with_an_etag() {
        let dir = TempDir::new();
        std::fs::write(dir.path().join("note.txt"), "hello").unwrap();

        let response = send(
            &app(),
            content_request(
                "/fs",
                serde_json::json!({ "path": dir.path().join("note.txt").display().to_string() }),
            ),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        let etag = response.headers()["etag"].to_str().unwrap().to_owned();
        let body = json_body(response).await;
        assert_eq!(body["content"], "hello");
        assert_eq!(body["size"], 5);
        assert_eq!(body["etag"], etag);
    }

    #[tokio::test]
    async fn content_rejects_bytes_that_are_not_utf8() {
        let dir = TempDir::new();
        std::fs::write(dir.path().join("blob.bin"), b"\xff").unwrap();

        let response = send(
            &app(),
            content_request(
                "/fs",
                serde_json::json!({ "path": dir.path().join("blob.bin").display().to_string() }),
            ),
        )
        .await;

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            json_body(response).await["error"]["code"],
            "invalid_request"
        );
    }

    #[tokio::test]
    async fn content_reports_a_directory_a_missing_file_and_a_malformed_body() {
        let dir = TempDir::new();
        let app = app();

        let response = send(
            &app,
            content_request(
                "/fs",
                serde_json::json!({ "path": dir.path().display().to_string() }),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(json_body(response).await["error"]["code"], "not_a_file");

        let response = send(
            &app,
            content_request(
                "/fs",
                serde_json::json!({ "path": dir.path().join("missing").display().to_string() }),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let response = send(
            &app,
            Request::builder()
                .method("QUERY")
                .uri("/fs?type=content")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            json_body(response).await["error"]["code"],
            "invalid_request"
        );
    }

    #[tokio::test]
    async fn content_reads_a_workspace_relative_path_and_confines_it() {
        let dir = TempDir::new();
        std::fs::write(dir.path().join("note.txt"), "hi").unwrap();
        let state = AppState::new(".");
        state
            .workspaces()
            .register("docs", &dir.root(), WorkspaceProperties::default())
            .await
            .unwrap();
        let app = router().with_state(state);

        let response = send(
            &app,
            content_request(
                "/workspaces/docs/fs",
                serde_json::json!({ "path": "note.txt" }),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["path"], "note.txt");
        assert_eq!(body["content"], "hi");

        // The same file is not addressable as an absolute path inside a workspace.
        let response = send(
            &app,
            content_request(
                "/workspaces/docs/fs",
                serde_json::json!({ "path": dir.path().join("note.txt").display().to_string() }),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn unimplemented_types_report_not_implemented() {
        let app = app();

        for (method, uri) in [
            ("QUERY", "/fs?type=list"),
            ("QUERY", "/fs?type=glob"),
            ("QUERY", "/fs?type=watch"),
            ("PUT", "/fs?type=sink"),
            ("PUT", "/fs?type=directory"),
            ("PATCH", "/fs?type=patch"),
            ("POST", "/fs?type=copy"),
            ("POST", "/fs?type=move"),
        ] {
            assert_eq!(
                call(&app, method, uri).await,
                StatusCode::NOT_IMPLEMENTED,
                "{method} {uri}"
            );
        }
    }

    #[tokio::test]
    async fn rejects_unknown_and_mismatched_types() {
        let app = app();

        assert_eq!(
            call(&app, "QUERY", "/fs?type=bogus").await,
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            call(&app, "QUERY", "/fs").await,
            StatusCode::UNPROCESSABLE_ENTITY
        );

        assert_eq!(
            call(&app, "PUT", "/fs?type=metadata").await,
            StatusCode::METHOD_NOT_ALLOWED
        );
        assert_eq!(
            call(&app, "QUERY", "/fs?type=file").await,
            StatusCode::METHOD_NOT_ALLOWED
        );

        assert_eq!(
            call(&app, "GET", "/fs").await,
            StatusCode::METHOD_NOT_ALLOWED
        );
    }

    #[tokio::test]
    async fn workspace_mutations_honor_the_access_property() {
        let dir = TempDir::new();
        let state = AppState::new(".");
        state
            .workspaces()
            .register(
                "sealed",
                &dir.root(),
                WorkspaceProperties {
                    access: WorkspaceAccess::ReadOnly,
                },
            )
            .await
            .unwrap();
        let app = router().with_state(state);

        for (method, uri) in [
            ("PUT", "/workspaces/sealed/fs?type=directory"),
            ("DELETE", "/workspaces/sealed/fs"),
        ] {
            assert_eq!(
                call(&app, method, uri).await,
                StatusCode::FORBIDDEN,
                "{method} {uri}"
            );
        }
    }

    #[tokio::test]
    async fn workspace_routes_resolve_the_registered_workspace() {
        let app = app();

        assert_eq!(
            call(&app, "QUERY", "/workspaces/missing/fs?type=metadata").await,
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn stream_returns_raw_bytes_and_headers() {
        let dir = TempDir::new();
        std::fs::write(dir.path().join("note.txt"), "hello").unwrap();

        let response = send(
            &app(),
            list_request(
                "/fs",
                "stream",
                serde_json::json!({ "path": dir.path().join("note.txt").display().to_string() }),
            ),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["content-length"].to_str().unwrap(), "5");
        assert_eq!(
            response.headers()["accept-ranges"].to_str().unwrap(),
            "none"
        );
        assert!(response.headers().contains_key("etag"));
        assert!(response.headers().contains_key("last-modified"));

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&bytes[..], b"hello");
    }

    #[tokio::test]
    async fn metadata_reports_the_resource_with_an_etag() {
        let dir = TempDir::new();
        std::fs::write(dir.path().join("note.txt"), "hello").unwrap();

        let response = send(
            &app(),
            list_request(
                "/fs",
                "metadata",
                serde_json::json!({ "path": dir.path().join("note.txt").display().to_string() }),
            ),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        let etag = response.headers()["etag"].to_str().unwrap().to_owned();
        let body = json_body(response).await;
        assert_eq!(body["kind"], "file");
        assert_eq!(body["name"], "note.txt");
        assert_eq!(body["size"], 5);
        assert_eq!(body["etag"], etag);
    }

    #[tokio::test]
    async fn realpath_access_and_lines_round_trip() {
        let dir = TempDir::new();
        let path = dir.path().join("note.txt");
        std::fs::write(&path, "a\nb\nc").unwrap();

        let response = send(
            &app(),
            list_request(
                "/fs",
                "realpath",
                serde_json::json!({ "path": dir.path().join("./note.txt").display().to_string() }),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let canonical = std::fs::canonicalize(&path).unwrap();
        assert_eq!(
            json_body(response).await["path"],
            canonical.display().to_string()
        );

        let response = send(
            &app(),
            list_request(
                "/fs",
                "access",
                serde_json::json!({ "path": path.display().to_string() }),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(json_body(response).await["readable"], true);

        let response = send(
            &app(),
            list_request(
                "/fs",
                "lines",
                serde_json::json!({ "path": path.display().to_string(), "offset": 1, "limit": 1 }),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["lines"], serde_json::json!(["b"]));
        assert_eq!(body["offset"], 1);
        assert_eq!(body["truncated"], true);
    }

    #[tokio::test]
    async fn put_file_creates_then_replaces() {
        let app = app();
        let dir = TempDir::new();
        let path = dir.path().join("note.txt").display().to_string();

        let response = send(
            &app,
            mutation_request(
                "PUT",
                "/fs",
                Some("file"),
                serde_json::json!({ "path": path.clone(), "content": "hello" }),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);
        assert!(response.headers().contains_key("etag"));
        assert_eq!(json_body(response).await["size"], 5);

        let response = send(
            &app,
            mutation_request(
                "PUT",
                "/fs",
                Some("file"),
                serde_json::json!({ "path": path, "content": "hi" }),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(json_body(response).await["size"], 2);
    }

    #[tokio::test]
    async fn put_symlink_then_delete_round_trips() {
        let app = app();
        let dir = TempDir::new();
        std::fs::write(dir.path().join("note.txt"), "hi").unwrap();
        let link = dir.path().join("link").display().to_string();

        let response = send(
            &app,
            mutation_request(
                "PUT",
                "/fs",
                Some("symlink"),
                serde_json::json!({ "path": link.clone(), "target": "note.txt" }),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = json_body(response).await;
        assert_eq!(body["kind"], "symlink");
        assert_eq!(body["target"], "note.txt");

        let response = send(
            &app,
            mutation_request("DELETE", "/fs", None, serde_json::json!({ "path": link })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn truncate_and_patch_metadata_apply_to_a_file() {
        let app = app();
        let dir = TempDir::new();
        let path = dir.path().join("note.txt");
        std::fs::write(&path, "hello").unwrap();

        let response = send(
            &app,
            mutation_request(
                "PATCH",
                "/fs",
                Some("truncate"),
                serde_json::json!({ "path": path.display().to_string(), "length": 2 }),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(json_body(response).await["size"], 2);

        {
            let response = send(
                &app,
                mutation_request(
                    "PATCH",
                    "/fs",
                    Some("metadata"),
                    serde_json::json!({ "path": path.display().to_string(), "mode": "0600" }),
                ),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(json_body(response).await["mode"], "0600");
        }
    }

    #[tokio::test]
    async fn delete_reports_missing_without_force_and_succeeds_with_it() {
        let app = app();
        let dir = TempDir::new();
        let missing = dir.path().join("missing").display().to_string();

        let response = send(
            &app,
            mutation_request(
                "DELETE",
                "/fs",
                None,
                serde_json::json!({ "path": missing.clone() }),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let response = send(
            &app,
            mutation_request(
                "DELETE",
                "/fs",
                None,
                serde_json::json!({ "path": missing, "force": true }),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }
}
