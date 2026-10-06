use std::sync::Arc;

use thiserror::Error;

use crate::events;
use crate::mcp;
use crate::plugin::{self, Plugins};
use crate::skill::Skills;

use super::Metadata;

/// The resource set a scope holds. Construction is whole: any class that fails
/// to load leaves the scope holding nothing.
#[derive(Clone)]
pub struct Resources {
    pub plugins: Plugins,
    pub mcps: Arc<mcp::Runtime>,
    pub skills: Skills,
    /// The scope's watcher, which every event stream of the scope subscribes
    /// to.
    pub events: Arc<events::Observer>,
}

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Plugin(#[from] plugin::Error),
    #[error(transparent)]
    Mcp(#[from] mcp::Error),
    #[error(transparent)]
    Watch(#[from] sandbox_toolkit_utils::watch::Error),
}

impl Resources {
    pub async fn new(metadata: &Metadata) -> Result<Self, Error> {
        // Plugins load first, and the components they declare are handed to the
        // two resources that present them.
        let plugins = Plugins::load(&metadata.root)?;
        let mcps = Arc::new(mcp::Runtime::new(&metadata.root, plugins.clone()).await?);
        let skills = Skills::new(&metadata.id, &metadata.root, plugins.clone());
        let events = Arc::new(
            events::Observer::new(&metadata.id, &metadata.root, plugins.clone(), Arc::clone(&mcps))
                .await?,
        );

        Ok(Self {
            plugins,
            mcps,
            skills,
            events,
        })
    }
}
