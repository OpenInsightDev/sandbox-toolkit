use std::convert::Infallible;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use bytes::Bytes;
use salvo::Server;
use salvo::extract::Metadata;
use salvo::http::body::ResBody;
use salvo::http::uri::Uri;
use salvo::http::{HeaderMap, HeaderValue, header};
use salvo::prelude::*;
use salvo::Extractible;
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;

use crate::events::{self, Event};
use crate::exec;
use crate::fs;
use crate::mcp;
use crate::plugin;
use crate::pty;
use crate::skill::{self, Skills};
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
    /// The uploads tusd stages, which `POST ?type=commit` moves onto a path.
    pub uploads: tus::Uploads,
}

impl AppState {
    pub fn new(
        registry: Registry,
        bin: PathBuf,
        tus: tus::Upstream,
        uploads: tus::Uploads,
    ) -> Self {
        Self {
            registry: Arc::new(registry),
            bin,
            pty: pty::Sessions::new(),
            tus,
            uploads,
        }
    }
}

/// The state every handler resolves through, injected into the depot by
/// [`router`].
pub(crate) fn app_state(depot: &Depot) -> Result<&Arc<AppState>, StatusError> {
    depot
        .get_typed::<Arc<AppState>>()
        .map_err(|_| StatusError::internal_server_error())
}

pub type ExtractWorkspace = Resolved;

impl<'ex> Extractible<'ex> for Resolved {
    fn metadata() -> &'static Metadata {
        static METADATA: Metadata = Metadata::new("Resolved");
        &METADATA
    }

    #[allow(refining_impl_trait)]
    async fn extract(req: &'ex mut Request, depot: &'ex mut Depot) -> Result<Self, StatusError> {
        let state = Arc::clone(app_state(depot)?);
        // A mount without `{workspace_id}` is unscoped, so it resolves to the global
        // workspace.
        let workspace_id = req
            .param::<String>("workspace_id")
            .unwrap_or_else(|| GLOBAL_WORKSPACE_ID.to_owned());

        state
            .registry
            .resolve(&workspace_id)
            .await
            .ok_or_else(StatusError::not_found)
    }
}

pub struct ExtractResources {
    global: Option<Arc<Resources>>,
    scoped: Option<Arc<Resources>>,
}

impl<'ex> Extractible<'ex> for ExtractResources {
    fn metadata() -> &'static Metadata {
        static METADATA: Metadata = Metadata::new("ExtractResources");
        &METADATA
    }

    #[allow(refining_impl_trait)]
    async fn extract(req: &'ex mut Request, depot: &'ex mut Depot) -> Result<Self, StatusError> {
        let workspace = Resolved::extract(req, depot).await?;
        let state = Arc::clone(app_state(depot)?);
        let scoped = workspace.resources().await;
        let global = match state.registry.get(GLOBAL_WORKSPACE_ID).await {
            Some(global) => global.resources().await,
            None => None,
        };

        if global.is_none() && scoped.is_none() {
            return Err(StatusError::not_found());
        }

        Ok(Self { global, scoped })
    }
}

impl<'ex> Extractible<'ex> for mcp::http::ExtractRuntime {
    fn metadata() -> &'static Metadata {
        static METADATA: Metadata = Metadata::new("ExtractRuntime");
        &METADATA
    }

    #[allow(refining_impl_trait)]
    async fn extract(req: &'ex mut Request, depot: &'ex mut Depot) -> Result<Self, StatusError> {
        let resources = ExtractResources::extract(req, depot).await?;

        Ok(Self::new(
            resources
                .global
                .as_ref()
                .map(|resources| Arc::clone(&resources.mcps)),
            resources
                .scoped
                .as_ref()
                .map(|resources| Arc::clone(&resources.mcps)),
            resources.scoped.as_ref().map(scope_events),
        ))
    }
}

impl<'ex> Extractible<'ex> for plugin::http::ExtractPlugins {
    fn metadata() -> &'static Metadata {
        static METADATA: Metadata = Metadata::new("ExtractPlugins");
        &METADATA
    }

    #[allow(refining_impl_trait)]
    async fn extract(req: &'ex mut Request, depot: &'ex mut Depot) -> Result<Self, StatusError> {
        let resources = ExtractResources::extract(req, depot).await?;

        Ok(Self::new(
            rescanned(resources.global.as_ref()).map_err(unloadable)?,
            rescanned(resources.scoped.as_ref()).map_err(unloadable)?,
            resources.scoped.as_ref().map(scope_events),
        ))
    }
}

/// The observer of the workspace a mount is scoped to, which owns every event
/// stream that mount answers.
fn scope_events(resources: &Arc<Resources>) -> Arc<events::Observer> {
    Arc::clone(&resources.events)
}

/// A scope's plugin directory as this request finds it: `Ok(None)` when the
/// scope holds no resources, and an error when its plugins no longer load. The
/// plugin mount reads the directory per request, so a plugin that was added,
/// removed, or broken on disk is answered from the directory itself.
fn rescanned(
    resources: Option<&Arc<Resources>>,
) -> Result<Option<plugin::Plugins>, plugin::Error> {
    match resources {
        Some(resources) => resources.plugins.rescan().map(Some),
        None => Ok(None),
    }
}

/// A plugin directory that stopped loading answers `404`, exactly as a load
/// failure at construction does.
fn unloadable(error: plugin::Error) -> StatusError {
    tracing::warn!(%error, "failed to rediscover plugins");
    StatusError::not_found()
}

impl<'ex> Extractible<'ex> for skill::http::ExtractSkills {
    fn metadata() -> &'static Metadata {
        static METADATA: Metadata = Metadata::new("ExtractSkills");
        &METADATA
    }

    #[allow(refining_impl_trait)]
    async fn extract(req: &'ex mut Request, depot: &'ex mut Depot) -> Result<Self, StatusError> {
        let resources = ExtractResources::extract(req, depot).await?;

        Ok(Self::new(
            rescanned_skills(resources.global.as_ref()).map_err(unloadable)?,
            rescanned_skills(resources.scoped.as_ref()).map_err(unloadable)?,
            resources.scoped.as_ref().map(scope_events),
        ))
    }
}

/// A scope's skills as this request finds them, the ones its plugins ship
/// included; `Ok(None)` when the scope holds no resources, and an error when
/// those plugins no longer load.
fn rescanned_skills(resources: Option<&Arc<Resources>>) -> Result<Option<Skills>, plugin::Error> {
    match resources {
        Some(resources) => resources.skills.rescan().map(Some),
        None => Ok(None),
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .hoop(salvo::affix_state::inject(Arc::new(state)))
        .hoop(salvo::logging::Logger::new())
        .push(Router::with_path("tus").push(tus::http::routes()))
        .push(Router::with_path("fs").push(fs::http::routes()))
        .push(Router::with_path("exec").push(exec::http::routes()))
        .push(Router::with_path("pty").push(pty::http::routes()))
        .push(Router::with_path("mcps").push(mcp::http::routes()))
        .push(Router::with_path("skills").push(skill::http::routes()))
        .push(Router::with_path("plugins").push(plugin::http::routes()))
        .push(Router::with_path("workspaces").push(crate::workspace::http::routes()))
        .push(
            Router::with_path("workspaces/{workspace_id}/fs").push(fs::http::routes()),
        )
        .push(
            Router::with_path("workspaces/{workspace_id}/exec").push(exec::http::routes()),
        )
        .push(Router::with_path("workspaces/{workspace_id}/pty").push(pty::http::routes()))
        .push(Router::with_path("workspaces/{workspace_id}/mcps").push(mcp::http::routes()))
        .push(
            Router::with_path("workspaces/{workspace_id}/skills").push(skill::http::routes()),
        )
        .push(
            Router::with_path("workspaces/{workspace_id}/plugins")
                .push(plugin::http::routes()),
        )
}

/// Serves until `shutdown` resolves and the requests in flight have been
/// answered.
pub async fn serve(
    address: (String, u16),
    state: AppState,
    shutdown: impl Future<Output = ()> + Send + 'static,
) {
    let acceptor = TcpListener::new(format!("{}:{}", address.0, address.1))
        .bind()
        .await;
    let mut server = Server::new(acceptor);
    // The pty attach is an HTTP/2 extended CONNECT, which hyper only accepts
    // once this is on.
    server.http2_mut().enable_connect_protocol();

    let handle = server.handle();
    tokio::spawn(async move {
        shutdown.await;
        handle.stop_graceful(None);
    });

    server.serve(router(state)).await;
}

/// The external scheme and host the request reached us through: a forwarded
/// header, then the request URI (h2 carries scheme and authority there; an h1
/// origin-form request only the path, so `Host` stands in). They stay header
/// values, which is the form the proxy forwards them in.
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

const EVENT_STREAM: &str = "text/event-stream";

/// Whether the request asked for a mount's event stream instead of its
/// document.
pub(crate) fn wants_events(req: &Request) -> bool {
    req.headers()
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains(EVENT_STREAM))
}

/// Answers a mount's event stream: the SSE content type, and a body that frames
/// every event reported after this subscription. `frame` shapes the payload, so
/// a resource's own mount and the workspace summary differ only there.
pub(crate) fn respond_events(
    res: &mut Response,
    events: broadcast::Receiver<Event>,
    frame: fn(&Event) -> Bytes,
) {
    res.headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(EVENT_STREAM));
    res.body(ResBody::stream(frames(events, frame)));
}

/// Frames a source's later events as they arrive. A subscriber the broadcaster
/// outran skips what it missed instead of ending the stream.
fn frames(
    events: broadcast::Receiver<Event>,
    frame: fn(&Event) -> Bytes,
) -> impl futures_util::Stream<Item = Result<Bytes, Infallible>> + Send + 'static {
    futures_util::stream::unfold(events, move |mut events| async move {
        loop {
            match events.recv().await {
                Ok(event) => return Some((Ok(frame(&event)), events)),
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => return None,
            }
        }
    })
}
