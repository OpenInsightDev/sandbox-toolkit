use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::extract::{FromRequestParts, Path};
use axum::http::StatusCode;
use axum::http::header;
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, Uri};
use axum::response::{IntoResponse, Response};
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;

use crate::exec;
use crate::mcp;
use crate::pty;
use crate::skill;
use crate::tus;
use crate::workspace::{GLOBAL_WORKSPACE_ID, Registry, Resolved, Resources};

#[derive(Clone)]
pub struct AppState {
    pub registry: Arc<Registry>,
    /// Where the embedded tools were materialized, the directory `exec` puts on
    /// the `PATH` of the commands it deploys.
    pub bin: PathBuf,
    pub pty: pty::Sessions,
    /// The tus sidecar's endpoint, which `/tus` relays to.
    pub tus: tus::Upstream,
}

impl AppState {
    pub fn new(registry: Registry, bin: PathBuf, tus: tus::Upstream) -> Self {
        Self {
            registry: Arc::new(registry),
            bin,
            pty: pty::Sessions::new(),
            tus,
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
        .nest("/tus", tus::http::routes())
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

/// Serves until `shutdown` resolves and the requests in flight have been
/// answered.
pub async fn serve(
    listener: TcpListener,
    state: AppState,
    shutdown: impl Future<Output = ()> + Send + 'static,
) {
    axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown)
        .await
}

/// The external scheme and host the request reached us through, resolved like
/// axum's `Scheme`/`Host` extractors: a forwarded header, then the request URI
/// (h2 carries scheme and authority there; an h1 origin-form request only the
/// path, so `Host` stands in). They stay header values, which is the form the
/// proxy forwards them in.
pub(crate) fn origin_parts(uri: &Uri, headers: &HeaderMap) -> (HeaderValue, HeaderValue) {
    let scheme = headers
        .get("x-forwarded-proto")
        .cloned()
        .or_else(|| {
            uri.scheme_str()
                .and_then(|scheme| HeaderValue::from_str(scheme).ok())
        })
        .unwrap_or_else(|| HeaderValue::from_static("http"));
    let host = headers
        .get(header::HOST)
        .cloned()
        .or_else(|| {
            uri.authority()
                .and_then(|authority| HeaderValue::from_str(authority.as_str()).ok())
        })
        // A request that names no host has none to describe, and an empty value
        // is what an upstream reads as absent.
        .unwrap_or_else(|| HeaderValue::from_static(""));

    (scheme, host)
}

/// The external origin (`scheme` + host) the request reached us through.
pub(crate) fn origin(uri: &Uri, headers: &HeaderMap) -> String {
    let (scheme, host) = origin_parts(uri, headers);
    let text = |value: &HeaderValue| value.to_str().unwrap_or_default().to_owned();

    format!("{}://{}", text(&scheme), text(&host))
}
