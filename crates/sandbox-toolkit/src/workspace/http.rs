use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde::Deserialize;

use crate::http::AppState;
use crate::workspace::{
    CreateWorkspaceRequest, Metadata, UpdateWorkspaceRequest, Workspace, WorkspaceError,
    WorkspaceList,
};

/// Named rather than positional: a sibling capture on the enclosing mount would
/// otherwise shift the fields.
#[derive(Debug, Deserialize)]
struct WorkspacePath {
    workspace_id: String,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list).post(create))
        .route("/{workspace_id}", get(one).patch(update).delete(remove))
}

async fn create(
    State(state): State<AppState>,
    Json(request): Json<CreateWorkspaceRequest>,
) -> Response {
    let metadata = match Metadata::new(request).await {
        Ok(metadata) => metadata,
        Err(error) => return error.into_response(),
    };

    let created = metadata.clone();
    let workspace = match Workspace::new(metadata).await {
        Ok(workspace) => workspace,
        Err(error) => return error.into_response(),
    };

    match state.registry.register(workspace).await {
        Ok(()) => (StatusCode::CREATED, Json(created)).into_response(),
        Err(error) => error.into_response(),
    }
}

async fn list(State(state): State<AppState>) -> Json<WorkspaceList> {
    Json(WorkspaceList::new(state.registry.list().await))
}

async fn one(State(state): State<AppState>, Path(path): Path<WorkspacePath>) -> Response {
    match state.registry.get(&path.workspace_id).await {
        Some(workspace) => Json(workspace.metadata().await).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn update(
    State(state): State<AppState>,
    Path(path): Path<WorkspacePath>,
    Json(request): Json<UpdateWorkspaceRequest>,
) -> Response {
    match state
        .registry
        .update_access(&path.workspace_id, request.access)
        .await
    {
        Ok(metadata) => Json(metadata).into_response(),
        Err(error) => error.into_response(),
    }
}

async fn remove(State(state): State<AppState>, Path(path): Path<WorkspacePath>) -> Response {
    match state.registry.remove(&path.workspace_id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => error.into_response(),
    }
}

impl IntoResponse for WorkspaceError {
    fn into_response(self) -> Response {
        let status = match self {
            WorkspaceError::InvalidId { .. } | WorkspaceError::InvalidRoot { .. } => {
                StatusCode::BAD_REQUEST
            }
            WorkspaceError::PermissionDenied { .. }
            | WorkspaceError::Immutable { .. }
            | WorkspaceError::ReadOnly { .. } => StatusCode::FORBIDDEN,
            WorkspaceError::AlreadyExists { .. } => StatusCode::CONFLICT,
            WorkspaceError::NotFound { .. } => StatusCode::NOT_FOUND,
            WorkspaceError::Watch(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };

        (status, self.to_string()).into_response()
    }
}
