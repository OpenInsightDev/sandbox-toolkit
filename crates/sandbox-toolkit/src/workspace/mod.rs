//! The workspace control plane.
//!
//! A workspace registers a remote absolute path under a caller-chosen id and is
//! the boundary every path-bearing operation is confined to; other resource
//! APIs reuse it, such as an MCP stdio entry that runs inside one. The wire
//! types live in [`model`] and are reused by the MCP tools defined in this module.
//!
//! The registry itself is [`WorkspaceRegistry`], owned by [`crate::AppState`];
//! these routes are its control plane, and [`WorkspaceRegistry::resolve`] is the
//! resolution API the other modules build on.

pub(crate) mod model;
pub(crate) mod registry;

pub(crate) use self::registry::WorkspaceRegistry;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing;
use axum::{Json, Router};

use self::model::{CreateWorkspaceRequest, Workspace, WorkspaceId, WorkspaceList};
use self::registry::WorkspaceError;
use crate::{AppError, AppState};
use rmcp::ErrorData;
use rmcp::handler::server::wrapper::{Json as McpJson, Parameters};
use rmcp::{tool, tool_router};

use crate::mcp::{ToolkitServer, not_implemented};

/// Build the workspace control-plane router.
pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/workspaces",
            routing::post(create_workspace).get(list_workspaces),
        )
        .route(
            "/workspaces/{workspace_id}",
            routing::get(get_workspace).delete(delete_workspace),
        )
}

/// Register a remote absolute path as a workspace.
async fn create_workspace(
    State(state): State<AppState>,
    Json(request): Json<CreateWorkspaceRequest>,
) -> Result<(StatusCode, Json<Workspace>), AppError> {
    let workspace = state
        .workspaces()
        .register(&request.id, &request.root)
        .await?;

    Ok((StatusCode::CREATED, Json(workspace)))
}

async fn list_workspaces(State(state): State<AppState>) -> Result<Json<WorkspaceList>, AppError> {
    Ok(Json(WorkspaceList::new(state.workspaces().list())))
}

async fn get_workspace(
    State(state): State<AppState>,
    Path(id): Path<WorkspaceId>,
) -> Result<Json<Workspace>, AppError> {
    Ok(Json(state.workspaces().get(&id.workspace_id)?))
}

async fn delete_workspace(
    State(state): State<AppState>,
    Path(id): Path<WorkspaceId>,
) -> Result<StatusCode, AppError> {
    state.workspaces().remove(&id.workspace_id)?;

    Ok(StatusCode::NO_CONTENT)
}

/// Map registry failures onto the shared HTTP error envelope.
impl From<WorkspaceError> for AppError {
    fn from(error: WorkspaceError) -> Self {
        match error {
            WorkspaceError::InvalidId { id, reason } => {
                Self::BadRequest(format!("invalid workspace id `{id}`: {reason}"))
            }
            WorkspaceError::ReservedId { id } => Self::WorkspaceIdReserved(id),
            WorkspaceError::InvalidRoot { root, reason } => {
                Self::BadRequest(format!("invalid workspace root `{root}`: {reason}"))
            }
            WorkspaceError::AlreadyExists { id } => {
                Self::Conflict(format!("workspace `{id}` already exists"))
            }
            WorkspaceError::NotFound { id } => Self::NotFound(format!("workspace `{id}`")),
        }
    }
}

#[tool_router(router = workspace_tools, vis = "pub(crate)")]
impl ToolkitServer {
    /// Register a remote absolute path as a workspace and return its handle.
    #[tool(name = "create_workspace")]
    async fn create_workspace(
        &self,
        Parameters(_request): Parameters<CreateWorkspaceRequest>,
    ) -> Result<McpJson<Workspace>, ErrorData> {
        Err(not_implemented("create_workspace"))
    }

    /// List the registered workspaces.
    #[tool(name = "list_workspaces")]
    async fn list_workspaces(&self) -> Result<McpJson<WorkspaceList>, ErrorData> {
        Err(not_implemented("list_workspaces"))
    }

    /// Fetch a workspace by id.
    #[tool(name = "get_workspace")]
    async fn get_workspace(
        &self,
        Parameters(_id): Parameters<WorkspaceId>,
    ) -> Result<McpJson<Workspace>, ErrorData> {
        Err(not_implemented("get_workspace"))
    }

    /// Delete a workspace by id.
    #[tool(name = "delete_workspace")]
    async fn delete_workspace(
        &self,
        Parameters(_id): Parameters<WorkspaceId>,
    ) -> Result<(), ErrorData> {
        Err(not_implemented("delete_workspace"))
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use serde_json::Value;
    use tower::ServiceExt;

    use super::registry::test_support::{TempDir, canonical};
    use super::*;

    /// Drive one request through the router and decode its JSON body.
    async fn call(app: &Router, request: Request<Body>) -> (StatusCode, Value) {
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();

        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        };

        (status, body)
    }

    fn create_request(id: &str, root: &str) -> Request<Body> {
        json_request(
            "POST",
            "/workspaces",
            &serde_json::json!({ "id": id, "root": root }),
        )
    }

    fn json_request(method: &str, uri: &str, body: &Value) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    fn get_workspace_request(id: &str) -> Request<Body> {
        Request::builder()
            .method("GET")
            .uri(format!("/workspaces/{id}"))
            .body(Body::empty())
            .unwrap()
    }

    fn list_workspaces_request() -> Request<Body> {
        Request::builder()
            .method("GET")
            .uri("/workspaces")
            .body(Body::empty())
            .unwrap()
    }

    fn delete_workspace_request(id: &str) -> Request<Body> {
        Request::builder()
            .method("DELETE")
            .uri(format!("/workspaces/{id}"))
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn workspace_endpoints_round_trip() {
        let state = AppState::new(".");
        let app = router().with_state(state.clone());
        let dir = TempDir::new();

        let (status, _) = call(&app, get_workspace_request("docs")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let (status, body) = call(&app, create_request("docs", &dir.root())).await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(body, serde_json::json!({ "id": "docs" }));

        // The router shares the registry with the rest of the process, which is
        // how other modules resolve a workspace id to its root.
        assert_eq!(
            state.workspaces().resolve("docs").unwrap(),
            canonical(dir.path())
        );

        let (status, body) = call(&app, list_workspaces_request()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body,
            serde_json::json!({ "workspaces": [{ "id": "docs" }] })
        );

        let (status, body) = call(&app, get_workspace_request("docs")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, serde_json::json!({ "id": "docs" }));

        let (status, _) = call(&app, delete_workspace_request("docs")).await;
        assert_eq!(status, StatusCode::NO_CONTENT);

        let (status, body) = call(&app, get_workspace_request("docs")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"]["code"].as_str(), Some("not_found"));
    }

    #[tokio::test]
    async fn create_reports_registration_failures() {
        let app = router().with_state(AppState::new("."));
        let dir = TempDir::new();

        let (status, body) = call(&app, create_request("Bad", &dir.root())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"]["code"].as_str(), Some("bad_request"));

        let (status, body) = call(&app, create_request("skill-docs", &dir.root())).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(
            body["error"]["code"].as_str(),
            Some("workspace_id_reserved")
        );

        let (status, body) = call(&app, create_request("docs", "relative/path")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"]["code"].as_str(), Some("bad_request"));

        let (status, _) = call(&app, create_request("docs", &dir.root())).await;
        assert_eq!(status, StatusCode::CREATED);
        let (status, body) = call(&app, create_request("docs", &dir.root())).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"]["code"].as_str(), Some("conflict"));
    }

    #[tokio::test]
    async fn delete_reports_unknown_ids() {
        let app = router().with_state(AppState::new("."));

        let (status, body) = call(&app, delete_workspace_request("missing")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"]["code"].as_str(), Some("not_found"));
    }
}
