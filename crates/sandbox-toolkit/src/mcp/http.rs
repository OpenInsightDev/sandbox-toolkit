use std::collections::BTreeSet;
use std::sync::Arc;

use salvo::http::ReqBody;
use salvo::prelude::*;
use tokio::sync::broadcast;

use crate::events::{self, Event};
use crate::http::{origin, respond_events, wants_events};

use super::model::{StreamableHttpMcpServer, StreamableHttpServerMcpConfig};
use super::proxy::Service;
use super::runtime::Runtime;

pub struct ExtractRuntime {
    global: Option<Arc<Runtime>>,
    workspace: Option<Arc<Runtime>>,
    events: Option<Arc<events::Observer>>,
}

impl ExtractRuntime {
    pub fn new(
        global: Option<Arc<Runtime>>,
        workspace: Option<Arc<Runtime>>,
        events: Option<Arc<events::Observer>>,
    ) -> Self {
        Self {
            global,
            workspace,
            events,
        }
    }

    /// The events the mount's scope reports; a scope that holds no resources
    /// reports none.
    pub fn stream(&self) -> broadcast::Receiver<Event> {
        events::subscribe(self.events.as_deref())
    }

    /// Whether any scope the mount presents stopped parsing its `mcp.json`.
    async fn broken(&self) -> bool {
        for runtime in [self.global.as_ref(), self.workspace.as_ref()]
            .into_iter()
            .flatten()
        {
            if runtime.broken().await {
                return true;
            }
        }
        false
    }

    /// The ids the mount presents, or `None` while a scope it presents stopped
    /// parsing its `mcp.json`.
    pub async fn ids(&self) -> Option<Vec<String>> {
        let mut ids = BTreeSet::new();
        if let Some(global) = &self.global {
            ids.extend(global.ids().await?);
        }
        if let Some(workspace) = &self.workspace {
            ids.extend(workspace.ids().await?);
        }
        Some(ids.into_iter().collect())
    }

    pub async fn get(&self, id: &str) -> Option<super::proxy::Service> {
        if self.broken().await {
            return None;
        }

        if let Some(workspace) = &self.workspace {
            if let Some(service) = workspace.get(id).await {
                return Some(service);
            }
        }
        match &self.global {
            Some(global) => global.get(id).await,
            None => None,
        }
    }
}

pub fn routes() -> Router {
    Router::new()
        .get(list)
        .push(Router::with_path("{mcp_id}").get(proxy).post(proxy).delete(proxy))
}

#[handler]
async fn list(
    runtime: ExtractRuntime,
    req: &mut Request,
    res: &mut Response,
) -> Result<(), StatusError> {
    if wants_events(req) {
        respond_events(res, runtime.stream(), events::resource_frame);

        return Ok(());
    }

    let base = format!("{}{}", origin(req.uri(), req.headers()), req.uri().path());
    // A scope that stopped parsing its `mcp.json` presents nothing.
    let Some(ids) = runtime.ids().await else {
        return Err(StatusError::not_found());
    };

    let servers = ids
        .into_iter()
        .map(|id| {
            let url = format!("{base}/{id}");
            (id, StreamableHttpMcpServer::new(url))
        })
        .collect();
    res.render(Json(StreamableHttpServerMcpConfig::new(servers)));

    Ok(())
}

#[handler]
async fn proxy(
    req: &mut Request,
    depot: &mut Depot,
    runtime: ExtractRuntime,
    res: &mut Response,
    ctrl: &mut FlowCtrl,
) -> Result<(), StatusError> {
    let id = req
        .param::<String>("mcp_id")
        .ok_or_else(StatusError::not_found)?;
    let Some(service) = runtime.get(&id).await else {
        return Err(StatusError::not_found());
    };

    // The rmcp service accepts any request body, so inference cannot pick the
    // adapter's body type on its own; pin it to salvo's `ReqBody`.
    <Service as TowerServiceCompat<ReqBody, _, _, _>>::compat(service)
        .handle(req, depot, res, ctrl)
        .await;

    Ok(())
}
