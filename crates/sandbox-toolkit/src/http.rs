use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::extract::{FromRequestParts, Path};
use axum::http::StatusCode;
use axum::http::header;
use axum::http::request::Parts;
use axum::http::{HeaderMap, Uri};
use axum::response::{IntoResponse, Response};
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;

use crate::exec;
use crate::mcp;
use crate::pty;
use crate::skill;
use crate::workspace::{GLOBAL_WORKSPACE_ID, Registry, Resolved, Resources};

#[derive(Clone)]
pub struct AppState {
    pub registry: Arc<Registry>,
    /// Where the embedded tools were materialized, the directory `exec` puts on
    /// the `PATH` of the commands it deploys.
    pub bin: PathBuf,
    pub pty: pty::Sessions,
}

impl AppState {
    pub fn new(registry: Registry, bin: PathBuf) -> Self {
        Self {
            registry: Arc::new(registry),
            bin,
            pty: pty::Sessions::new(),
        }
    }
}

pub type ExtractWorkspace = Resolved;

impl FromRequestParts<AppState> for Resolved {
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
            .resolve(&workspace_id)
            .await
            .ok_or_else(|| StatusCode::NOT_FOUND.into_response())
    }
}

pub struct ExtractResources {
    global: Option<Arc<Resources>>,
    scoped: Option<Arc<Resources>>,
}

impl FromRequestParts<AppState> for ExtractResources {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let workspace = ExtractWorkspace::from_request_parts(parts, state).await?;
        let scoped = workspace.resources().await;
        let global = match state.registry.get(GLOBAL_WORKSPACE_ID).await {
            Some(global) => global.resources().await,
            None => None,
        };

        if global.is_none() && scoped.is_none() {
            return Err(StatusCode::NOT_FOUND.into_response());
        }

        Ok(Self { global, scoped })
    }
}

impl FromRequestParts<AppState> for mcp::http::ExtractRuntime {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let resources = ExtractResources::from_request_parts(parts, state).await?;

        Ok(Self::new(
            resources
                .global
                .as_ref()
                .map(|resources| Arc::clone(&resources.mcps)),
            resources
                .scoped
                .as_ref()
                .map(|resources| Arc::clone(&resources.mcps)),
        ))
    }
}

impl FromRequestParts<AppState> for skill::http::ExtractSkills {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let resources = ExtractResources::from_request_parts(parts, state).await?;

        Ok(Self::new(
            resources
                .global
                .as_ref()
                .map(|resources| resources.skills.clone()),
            resources
                .scoped
                .as_ref()
                .map(|resources| resources.skills.clone()),
        ))
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .nest("/exec", exec::http::routes())
        .nest("/pty", pty::http::routes())
        .nest("/mcps", mcp::http::routes())
        .nest("/skills", skill::http::routes())
        .nest("/workspaces/{workspace_id}/exec", exec::http::routes())
        .nest("/workspaces/{workspace_id}/pty", pty::http::routes())
        .nest("/workspaces/{workspace_id}/mcps", mcp::http::routes())
        .nest("/workspaces/{workspace_id}/skills", skill::http::routes())
        .nest("/workspaces", crate::workspace::http::routes())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

pub async fn serve(listener: TcpListener, state: AppState) -> std::io::Result<()> {
    match axum::serve(listener, router(state)).await {}
}

/// The external origin (`scheme` + host) the request reached us through, resolved
/// like axum's `Scheme`/`Host` extractors: a forwarded header, then the request
/// URI (h2 carries scheme and authority there; an h1 origin-form request only the
/// path, so `Host` stands in).
pub(crate) fn origin(uri: &Uri, headers: &HeaderMap) -> String {
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .or_else(|| uri.scheme_str())
        .unwrap_or("http");
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .or_else(|| uri.authority().map(|authority| authority.as_str()))
        .unwrap_or_default();

    format!("{scheme}://{host}")
}
