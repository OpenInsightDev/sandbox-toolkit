use std::sync::Arc;

use salvo::extract::JsonBody;
use salvo::http::StatusCode;
use salvo::prelude::*;

use crate::events;
use crate::http::{app_state, respond_events, wants_events};
use crate::workspace::{
    CreateWorkspaceRequest, Metadata, UpdateWorkspaceRequest, Workspace, WorkspaceError,
    WorkspaceList,
};

pub fn routes() -> Router {
    Router::new()
        .get(list)
        .post(create)
        .push(Router::with_path("{workspace_id}").get(one).patch(update).delete(remove))
}

#[handler]
async fn create(
    depot: &mut Depot,
    body: JsonBody<CreateWorkspaceRequest>,
) -> Result<(StatusCode, Json<Metadata>), StatusError> {
    let state = app_state(depot)?;
    let metadata = Metadata::new(body.0).await.map_err(StatusError::from)?;
    let created = metadata.clone();
    let workspace = Workspace::new(metadata).await.map_err(StatusError::from)?;

    state
        .registry
        .register(workspace)
        .await
        .map_err(StatusError::from)?;

    Ok((StatusCode::CREATED, Json(created)))
}

#[handler]
async fn list(
    req: &mut Request,
    depot: &mut Depot,
    res: &mut Response,
) -> Result<(), StatusError> {
    let state = app_state(depot)?;
    if wants_events(req) {
        respond_events(res, state.registry.subscribe(), events::resource_frame);

        return Ok(());
    }

    res.render(Json(WorkspaceList::new(state.registry.list().await)));

    Ok(())
}

#[handler]
async fn one(
    req: &mut Request,
    depot: &mut Depot,
    res: &mut Response,
) -> Result<(), StatusError> {
    let state = app_state(depot)?;
    let id = workspace_id(req)?;
    let Some(workspace) = state.registry.resolve(&id).await else {
        return Err(StatusError::not_found());
    };

    if wants_events(req) {
        let observer = workspace
            .resources()
            .await
            .map(|resources| Arc::clone(&resources.events));
        respond_events(res, events::subscribe(observer.as_deref()), events::summary_frame);

        return Ok(());
    }

    res.render(Json(workspace.metadata().await));

    Ok(())
}

#[handler]
async fn update(
    req: &mut Request,
    depot: &mut Depot,
    body: JsonBody<UpdateWorkspaceRequest>,
) -> Result<Json<Metadata>, StatusError> {
    let state = app_state(depot)?;
    let id = workspace_id(req)?;

    state
        .registry
        .update_access(&id, body.0.access)
        .await
        .map(Json)
        .map_err(StatusError::from)
}

#[handler]
async fn remove(req: &mut Request, depot: &mut Depot) -> Result<StatusCode, StatusError> {
    let state = app_state(depot)?;
    let id = workspace_id(req)?;

    state
        .registry
        .remove(&id)
        .await
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(StatusError::from)
}

fn workspace_id(req: &Request) -> Result<String, StatusError> {
    req.param::<String>("workspace_id")
        .ok_or_else(StatusError::not_found)
}

impl From<WorkspaceError> for StatusError {
    fn from(error: WorkspaceError) -> Self {
        let status = match &error {
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

        StatusError::from_code(status)
            .unwrap_or_else(StatusError::internal_server_error)
            .brief(error.to_string())
    }
}
