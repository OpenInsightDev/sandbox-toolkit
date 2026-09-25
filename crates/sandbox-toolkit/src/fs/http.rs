//! The method selects the operation class and `type` the exact operation, so
//! `QUERY`, `PUT`, `PATCH` and `POST` each dispatch on it while `DELETE` has none.
//! Every handler is a stub: the file-system work behind each `type` is unwritten.

use std::collections::HashMap;

use axum::Router;
use axum::extract::{FromRequestParts, Path, Query};
use axum::http::Method;
use axum::http::request::Parts;
use axum::response::Response;
use axum::routing::{self, MethodRouter};
use serde::Deserialize;

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
                workspace: Some(state.workspaces().workspace(id)?),
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
    _target: FsTarget,
    Query(query): Query<ResourceQuery>,
) -> Result<Response, AppError> {
    match parse_type::<QueryType>(&query, &Method::QUERY)? {
        QueryType::Content => Err(not_implemented("QUERY ?type=content")),
        QueryType::Stream => Err(not_implemented("QUERY ?type=stream")),
        QueryType::Metadata => Err(not_implemented("QUERY ?type=metadata")),
        QueryType::List => Err(not_implemented("QUERY ?type=list")),
        QueryType::Glob => Err(not_implemented("QUERY ?type=glob")),
        QueryType::Realpath => Err(not_implemented("QUERY ?type=realpath")),
        QueryType::Access => Err(not_implemented("QUERY ?type=access")),
        QueryType::Lines => Err(not_implemented("QUERY ?type=lines")),
        QueryType::Watch => Err(not_implemented("QUERY ?type=watch")),
    }
}

async fn put_endpoint(
    target: FsTarget,
    Query(query): Query<ResourceQuery>,
) -> Result<Response, AppError> {
    target.require_writable()?;

    match parse_type::<PutType>(&query, &Method::PUT)? {
        PutType::File => Err(not_implemented("PUT ?type=file")),
        PutType::Sink => Err(not_implemented("PUT ?type=sink")),
        PutType::Directory => Err(not_implemented("PUT ?type=directory")),
        PutType::Symlink => Err(not_implemented("PUT ?type=symlink")),
    }
}

async fn patch_endpoint(
    target: FsTarget,
    Query(query): Query<ResourceQuery>,
) -> Result<Response, AppError> {
    target.require_writable()?;

    match parse_type::<PatchType>(&query, &Method::PATCH)? {
        PatchType::Metadata => Err(not_implemented("PATCH ?type=metadata")),
        PatchType::Patch => Err(not_implemented("PATCH ?type=patch")),
        PatchType::Truncate => Err(not_implemented("PATCH ?type=truncate")),
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

async fn delete_endpoint(target: FsTarget) -> Result<Response, AppError> {
    target.require_writable()?;

    Err(not_implemented("DELETE"))
}

fn not_implemented(operation: &'static str) -> AppError {
    AppError::NotImplemented(operation)
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

    #[tokio::test]
    async fn every_type_is_wired_to_its_method() {
        let app = app();

        for (method, uri) in [
            ("QUERY", "/fs?type=content"),
            ("QUERY", "/fs?type=stream"),
            ("QUERY", "/fs?type=metadata"),
            ("QUERY", "/fs?type=list"),
            ("QUERY", "/fs?type=glob"),
            ("QUERY", "/fs?type=realpath"),
            ("QUERY", "/fs?type=access"),
            ("QUERY", "/fs?type=lines"),
            ("QUERY", "/fs?type=watch"),
            ("PUT", "/fs?type=file"),
            ("PUT", "/fs?type=sink"),
            ("PUT", "/fs?type=directory"),
            ("PUT", "/fs?type=symlink"),
            ("PATCH", "/fs?type=metadata"),
            ("PATCH", "/fs?type=patch"),
            ("PATCH", "/fs?type=truncate"),
            ("POST", "/fs?type=copy"),
            ("POST", "/fs?type=move"),
            ("DELETE", "/fs"),
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
}
