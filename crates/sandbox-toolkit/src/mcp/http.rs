use std::collections::BTreeSet;
use std::sync::Arc;

use salvo::http::ReqBody;
use salvo::prelude::*;

use crate::http::origin;

use super::model::{StreamableHttpMcpServer, StreamableHttpServerMcpConfig};
use super::proxy::Service;
use super::runtime::Runtime;

pub struct ExtractRuntime {
    global: Option<Arc<Runtime>>,
    workspace: Option<Arc<Runtime>>,
}

impl ExtractRuntime {
    pub fn new(global: Option<Arc<Runtime>>, workspace: Option<Arc<Runtime>>) -> Self {
        Self { global, workspace }
    }

    /// The ids the mount presents, global's and the workspace's merged.
    pub async fn ids(&self) -> Vec<String> {
        let mut ids = BTreeSet::new();
        if let Some(global) = &self.global {
            ids.extend(global.ids().await);
        }
        if let Some(workspace) = &self.workspace {
            ids.extend(workspace.ids().await);
        }

        ids.into_iter().collect()
    }

    pub async fn get(&self, id: &str) -> Option<super::proxy::Service> {
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
async fn list(runtime: ExtractRuntime, req: &mut Request) -> Result<Json<StreamableHttpServerMcpConfig>, StatusError> {
    let base = format!("{}{}", origin(req.uri(), req.headers()), req.uri().path());
    let servers = runtime
        .ids()
        .await
        .into_iter()
        .map(|id| {
            let url = format!("{base}/{id}");
            (id, StreamableHttpMcpServer::new(url))
        })
        .collect();

    Ok(Json(StreamableHttpServerMcpConfig::new(servers)))
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
