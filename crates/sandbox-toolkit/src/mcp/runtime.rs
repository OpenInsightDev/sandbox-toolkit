use std::{collections::HashMap, path::Path, path::PathBuf};

use thiserror::Error;
use tokio::sync::RwLock;

use crate::events::Sources;
use crate::plugin::Plugins;

use super::config::{self};
use super::proxy::{Proxies, Proxy, Service};

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Config(#[from] config::Error),
}

enum State {
    Ready {
        proxies: Proxies,
        sources: Sources,
    },
    /// The configuration stopped parsing, so the scope declares nothing it can
    /// serve; the entries the last successful load produced are gone with it.
    Broken,
}

/// The entries a scope serves.
///
/// The scope's [`Observer`](crate::events::Observer) reloads this shared state,
/// so the entries a mount serves and the events it reports come from one scan.
pub struct Runtime {
    root: PathBuf,
    state: RwLock<State>,
}

impl Runtime {
    pub async fn new(root: impl Into<PathBuf>, plugins: Plugins) -> Result<Self, Error> {
        let root = root.into();
        let state = RwLock::new(Self::load(&root, &plugins).await?);

        Ok(Self { root, state })
    }

    async fn load(root: &Path, plugins: &Plugins) -> Result<State, Error> {
        let mut proxies = Proxies::new();
        let mut sources = Sources::new();

        for entry in config::entries(root, plugins).await? {
            sources.insert(entry.id.clone(), entry.digest);
            proxies.insert(entry.id.clone(), Proxy::new(entry, HashMap::new()));
        }

        Ok(State::Ready { proxies, sources })
    }
}

impl Runtime {
    /// Re-derives the entries from `plugins`, replacing the state; `None` means
    /// the scope's plugins no longer load, which leaves the scope declaring
    /// nothing, exactly as a load failure does.
    ///
    /// Returns each id with the content of the `mcp.json` it came from, empty
    /// while the configuration does not parse.
    pub async fn reload(&self, plugins: Option<&Plugins>) -> Sources {
        let next = match plugins {
            Some(plugins) => match Self::load(&self.root, plugins).await {
                Ok(state) => state,
                Err(error) => {
                    tracing::warn!(%error, "failed to reload mcp.json");
                    State::Broken
                }
            },
            None => State::Broken,
        };

        let sources = sources(&next);
        let mut state = self.state.write().await;
        // Any change under `.agents` reloads, so an unchanged `mcp.json` keeps
        // the entries that are already serving: dropping a proxy would end the
        // sessions connected through it.
        if !unchanged(&state, &next) {
            *state = next;
        }
        drop(state);

        sources
    }

    /// What the current state declares, without reloading it.
    pub async fn sources(&self) -> Sources {
        let state = self.state.read().await;

        sources(&state)
    }

    /// Whether the configuration stopped parsing; a scope that does not parse
    /// presents no entries, so its mounts answer `404`.
    pub async fn broken(&self) -> bool {
        matches!(&*self.state.read().await, State::Broken)
    }

    pub async fn get(&self, id: &str) -> Option<Service> {
        match &*self.state.read().await {
            State::Ready { proxies, .. } => proxies.get(id).map(|proxy| proxy.service.clone()),
            State::Broken => None,
        }
    }

    /// The ids the configuration declares, or `None` while it stopped parsing.
    pub async fn ids(&self) -> Option<Vec<String>> {
        match &*self.state.read().await {
            State::Ready { proxies, .. } => Some(proxies.keys().cloned().collect()),
            State::Broken => None,
        }
    }
}

fn sources(state: &State) -> Sources {
    match state {
        State::Ready { sources, .. } => sources.clone(),
        State::Broken => Sources::new(),
    }
}

/// Whether a reload found what the scope already serves. Only two `Ready`
/// states are compared by their entries: an empty `Ready` and a `Broken`
/// declare the same ids, and are not the same state.
fn unchanged(current: &State, next: &State) -> bool {
    match (current, next) {
        (State::Ready { sources: was, .. }, State::Ready { sources, .. }) => was == sources,
        (State::Broken, State::Broken) => true,
        _ => false,
    }
}
