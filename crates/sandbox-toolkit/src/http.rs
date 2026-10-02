use std::collections::HashMap;
use std::sync::Arc;

use axum::Router;
use axum::extract::{FromRequestParts, Path};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;

use crate::mcp;
use crate::workspace::{GLOBAL_WORKSPACE_ID, Registry, Workspace};

#[derive(Clone)]
pub struct AppState {
    pub registry: Arc<Registry>,
}

impl AppState {
    pub fn new(registry: Registry) -> Self {
        Self {
            registry: Arc::new(registry),
        }
    }
}

pub type ExtractWorkspace = Arc<Workspace>;

impl FromRequestParts<AppState> for ExtractWorkspace {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // A mount without `{workspace_id}` is unscoped, so it resolves to the global
        // workspace. Captures are read through a map because a struct extractor fails
        // to deserialize an absent capture instead of reporting it as optional.
        let Path(captures) = Path::<HashMap<String, String>>::from_request_parts(parts, state)
            .await
            .map_err(|rejection| rejection.into_response())?;
        let workspace_id = captures
            .get("workspace_id")
            .cloned()
            .unwrap_or_else(|| GLOBAL_WORKSPACE_ID.to_owned());

        state
            .registry
            .get(&workspace_id)
            .await
            .ok_or_else(|| StatusCode::NOT_FOUND.into_response())
    }
}

impl FromRequestParts<AppState> for mcp::http::ExtractRuntime {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let workspace = ExtractWorkspace::from_request_parts(parts, state).await?;

        let global = state.registry.get(GLOBAL_WORKSPACE_ID).await;
        let global = match global {
            Some(global) => global.mcps().await,
            None => None,
        };
        let scoped = workspace.mcps().await;

        if global.is_none() && scoped.is_none() {
            return Err(StatusCode::NOT_FOUND.into_response());
        }

        Ok(Self::new(global, scoped))
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .nest("/mcps", mcp::http::routes())
        .nest("/workspaces/{workspace_id}/mcps", mcp::http::routes())
        .nest("/workspaces", crate::workspace::http::routes())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

pub async fn serve(listener: TcpListener, state: AppState) -> std::io::Result<()> {
    match axum::serve(listener, router(state)).await {}
}
