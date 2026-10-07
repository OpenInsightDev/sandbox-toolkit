use std::{collections::HashMap, path::Path, path::PathBuf};

use thiserror::Error;
use tokio::sync::RwLock;

use crate::plugin::Plugins;

use super::config::{self};
use super::proxy::{Proxies, Proxy, Service};

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Config(#[from] config::Error),
}

/// The entries a scope serves.
pub struct Runtime {
    state: RwLock<Proxies>,
}

impl Runtime {
    pub async fn new(root: impl Into<PathBuf>, plugins: Plugins) -> Result<Self, Error> {
        let root = root.into();
        let state = RwLock::new(Self::load(&root, &plugins).await?);

        Ok(Self { state })
    }

    async fn load(root: &Path, plugins: &Plugins) -> Result<Proxies, Error> {
        let proxies = config::entries(root, plugins)
            .await?
            .into_iter()
            .map(|entry| (entry.id.clone(), Proxy::new(entry, HashMap::new())))
            .collect();

        Ok(proxies)
    }

    pub async fn get(&self, id: &str) -> Option<Service> {
        self.state
            .read()
            .await
            .get(id)
            .map(|proxy| proxy.service.clone())
    }

    /// The ids the configuration declares.
    pub async fn ids(&self) -> Vec<String> {
        self.state.read().await.keys().cloned().collect()
    }
}
