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

enum State {
    Ready(Proxies),
    /// The configuration stopped parsing, so the scope declares nothing it can
    /// serve; the entries the last successful load produced are gone with it.
    Broken,
}

pub struct Runtime {
    watch: Watch,
    state: Arc<RwLock<State>>,

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
        let state = Arc::new(RwLock::new(State::Ready(Self::load(&root, &env).await?)));

        let task_state = Arc::clone(&state);
        let handle = tokio::spawn(async move {
            while let Ok(Ok(event)) = events.recv().await {
                match event.kind {
                    EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => {
                        let next = match Self::load(&root, &env).await {
                            Ok(proxies) => State::Ready(proxies),
                            Err(error) => {
                                tracing::warn!(%error, "failed to reload mcp.json");
                                State::Broken
                            }
                        };
                        *task_state.write().await = next;
                    }
                    EventKind::Access(_) | EventKind::Any | EventKind::Other => {}
                }
            }
        });

        Ok(Self {
            watch,
            state,
            handle,
        })
    }
}

impl Runtime {
    /// Whether the configuration stopped parsing; a scope that does not parse
    /// presents no entries, so its mounts answer `404`.
    pub async fn broken(&self) -> bool {
        matches!(&*self.state.read().await, State::Broken)
    }

    pub async fn get(&self, id: &str) -> Option<Service> {
        match &*self.state.read().await {
            State::Ready(mcps) => mcps.get(id).map(|proxy| proxy.service.clone()),
            State::Broken => None,
        }
    }

    /// The ids the configuration declares, or `None` while it stopped parsing.
    pub async fn ids(&self) -> Option<Vec<String>> {
        match &*self.state.read().await {
            State::Ready(mcps) => Some(mcps.keys().cloned().collect()),
            State::Broken => None,
        }
    }
}
