use std::{collections::HashMap, ffi::OsString, path::Path, path::PathBuf, sync::Arc};

use notify::EventKind;
use sandbox_toolkit_utils::watch::Watch;
use thiserror::Error;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;

use crate::path::{AGENTS_DIR, MCP_JSON};

use super::config::{self};
use super::proxy::{Proxies, Proxy, Service};

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Watch(#[from] sandbox_toolkit_utils::watch::Error),
    #[error(transparent)]
    Config(#[from] config::Error),
}

pub struct Runtime {
    watch: Watch,
    mcps: Arc<RwLock<Proxies>>,

    handle: JoinHandle<()>,
}

impl Runtime {
    async fn load(root: &Path, env: &HashMap<String, OsString>) -> Result<Proxies, Error> {
        let proxies = config::entries(root)
            .await?
            .into_iter()
            .map(|entry| (entry.id.clone(), Proxy::new(entry, env.clone())))
            .collect();

        Ok(proxies)
    }

    pub async fn new(path: impl Into<PathBuf>) -> Result<Self, Error> {
        let root = path.into();
        let agents_dir = root.join(AGENTS_DIR);

        let watch = Watch::new(&agents_dir.join(MCP_JSON)).await?;

        let mut events = watch.subscribe();

        let env = HashMap::<String, OsString>::new();
        let mcps = Self::load(&root, &env).await?;
        let mcps = Arc::new(RwLock::new(mcps));

        let task_mcps = Arc::clone(&mcps);
        let handle = tokio::spawn(async move {
            while let Ok(Ok(event)) = events.recv().await {
                match event.kind {
                    EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => {
                        let Ok(next) = Self::load(&root, &env).await else {
                            continue;
                        };
                        let mut proxies = task_mcps.write().await;
                        *proxies = next;
                    }
                    EventKind::Access(_) | EventKind::Any | EventKind::Other => {}
                }
            }
        });

        Ok(Self {
            watch,
            mcps,
            handle,
        })
    }
}

impl Runtime {
    pub async fn get(&self, id: &str) -> Option<Service> {
        self.mcps
            .read()
            .await
            .get(id)
            .map(|proxy| proxy.service.clone())
    }

    pub async fn ids(&self) -> Vec<String> {
        self.mcps.read().await.keys().cloned().collect()
    }
}
