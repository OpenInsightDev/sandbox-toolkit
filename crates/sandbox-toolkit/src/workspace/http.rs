use std::collections::HashMap;

use axum::extract::{FromRequestParts, Path, State};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::routing;
use axum::{Json, Router};

use super::model::{CreateWorkspaceRequest, WorkspaceHandle, WorkspaceId, WorkspaceList};
use super::registry::{WorkspaceError, validate_id};
use crate::{AppError, AppState};

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

/// Extraction is the parsing boundary, so a malformed id is reported as
/// `bad_request` before any lookup and never falls through to `not_found`.
impl FromRequestParts<AppState> for WorkspaceId {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let Path(captures): Path<HashMap<String, String>> = Path::from_request_parts(parts, state)
            .await
            .map_err(|_| AppError::BadRequest("invalid workspace path".to_owned()))?;

        let workspace_id = captures
            .get("workspace_id")
            .ok_or_else(|| AppError::BadRequest("missing workspace id".to_owned()))?;

        validate_id(workspace_id)?;

        Ok(Self {
            workspace_id: workspace_id.clone(),
        })
    }
}

async fn create_workspace(
    State(state): State<AppState>,
    Json(request): Json<CreateWorkspaceRequest>,
) -> Result<(StatusCode, Json<WorkspaceHandle>), AppError> {
    let workspace = state
        .workspaces()
        .register(&request.id, &request.root, request.properties)
        .await?;

    Ok((StatusCode::CREATED, Json(workspace)))
}

async fn list_workspaces(State(state): State<AppState>) -> Result<Json<WorkspaceList>, AppError> {
    Ok(Json(WorkspaceList::new(state.workspaces().list())))
}

async fn get_workspace(
    State(state): State<AppState>,
    id: WorkspaceId,
) -> Result<Json<WorkspaceHandle>, AppError> {
    Ok(Json(state.workspaces().get(&id.workspace_id)?))
}

async fn delete_workspace(
    State(state): State<AppState>,
    id: WorkspaceId,
) -> Result<StatusCode, AppError> {
    state.workspaces().remove(&id.workspace_id)?;

    Ok(StatusCode::NO_CONTENT)
}

impl From<WorkspaceError> for AppError {
    fn from(error: WorkspaceError) -> Self {
        match error {
            WorkspaceError::InvalidId { id } => {
                Self::BadRequest(format!("invalid workspace id `{id}`"))
            }
            WorkspaceError::InvalidRoot { root, reason } => {
                Self::BadRequest(format!("invalid workspace root `{root}`: {reason}"))
            }
            WorkspaceError::AlreadyExists { id } => {
                Self::Conflict(format!("workspace `{id}` already exists"))
            }
            WorkspaceError::NotFound { id } => Self::NotFound(format!("workspace `{id}`")),
            WorkspaceError::ReadOnly { id } => Self::ReadOnlyWorkspace(id),
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use serde_json::Value;
    use tower::ServiceExt;

    use crate::workspace::registry::test_support::{TempDir, canonical};

    use super::*;

    /// Drive one request through the router and decode its JSON body.
    ///
    /// An extractor rejection carries a plain text body rather than the JSON
    /// envelope, so a body that does not parse decodes to [`Value::Null`].
    async fn call(app: &Router, request: Request<Body>) -> (StatusCode, Value) {
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();

        let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);

        (status, body)
    }

    fn create_request(id: &str, root: &str) -> Request<Body> {
        json_request(
            "POST",
            "/workspaces",
            &serde_json::json!({ "id": id, "root": root }),
        )
    }

    fn create_request_with_properties(id: &str, root: &str, properties: Value) -> Request<Body> {
        json_request(
            "POST",
            "/workspaces",
            &serde_json::json!({ "id": id, "root": root, "properties": properties }),
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
        assert_eq!(
            body,
            serde_json::json!({ "id": "docs", "properties": { "access": "read-write" } })
        );

        // The router shares the registry with the rest of the process, which is
        // how other modules resolve a workspace id to its root.
        assert_eq!(
            state.workspaces().workspace("docs").unwrap().root(),
            canonical(dir.path())
        );

        let (status, body) = call(&app, list_workspaces_request()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body,
            serde_json::json!({
                "workspaces": [
                    { "id": "docs", "properties": { "access": "read-write" } }
                ]
            })
        );

        let (status, body) = call(&app, get_workspace_request("docs")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body,
            serde_json::json!({ "id": "docs", "properties": { "access": "read-write" } })
        );

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
    async fn create_echoes_the_requested_access_mode() {
        let app = router().with_state(AppState::new("."));
        let dir = TempDir::new();

        let (status, body) = call(
            &app,
            create_request_with_properties(
                "sealed",
                &dir.root(),
                serde_json::json!({ "access": "read-only" }),
            ),
        )
        .await;

        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(
            body,
            serde_json::json!({ "id": "sealed", "properties": { "access": "read-only" } })
        );
    }

    #[tokio::test]
    async fn create_rejects_an_unknown_access_mode() {
        let app = router().with_state(AppState::new("."));
        let dir = TempDir::new();

        let (status, _) = call(
            &app,
            create_request_with_properties(
                "docs",
                &dir.root(),
                serde_json::json!({ "access": "readonly" }),
            ),
        )
        .await;

        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn get_and_delete_reject_malformed_ids() {
        let app = router().with_state(AppState::new("."));

        for method in ["GET", "DELETE"] {
            let request = Request::builder()
                .method(method)
                .uri("/workspaces/Bad")
                .body(Body::empty())
                .unwrap();

            let (status, body) = call(&app, request).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{method}");
            assert_eq!(
                body["error"]["code"].as_str(),
                Some("bad_request"),
                "{method}"
            );
        }
    }

    #[tokio::test]
    async fn delete_reports_unknown_ids() {
        let app = router().with_state(AppState::new("."));

        let (status, body) = call(&app, delete_workspace_request("missing")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"]["code"].as_str(), Some("not_found"));
    }
}
