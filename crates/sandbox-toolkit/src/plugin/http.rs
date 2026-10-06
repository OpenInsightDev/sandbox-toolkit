use std::collections::BTreeMap;
use std::sync::Arc;

use salvo::prelude::*;
use tokio::sync::broadcast;

use crate::events::{self, Event};
use crate::http::{respond_events, wants_events};

use super::discover::{Plugin, Plugins};
use super::model::PluginMetadata;

pub struct ExtractPlugins {
    global: Option<Plugins>,
    scoped: Option<Plugins>,
    events: Option<Arc<events::Observer>>,
}

impl ExtractPlugins {
    pub fn new(
        global: Option<Plugins>,
        scoped: Option<Plugins>,
        events: Option<Arc<events::Observer>>,
    ) -> Self {
        Self {
            global,
            scoped,
            events,
        }
    }

    /// The events the mount's scope reports; a scope that holds no resources
    /// reports none.
    pub fn stream(&self) -> broadcast::Receiver<Event> {
        events::subscribe(self.events.as_deref())
    }

    pub fn get(&self, id: &str) -> Option<&Plugin> {
        for plugins in [self.scoped.as_ref(), self.global.as_ref()]
            .into_iter()
            .flatten()
        {
            if let Some(plugin) = plugins.get(id) {
                return Some(plugin);
            }
        }

        None
    }

    /// The plugins the mount presents, merged by id in id order, a plugin the
    /// workspace holds winning over the global one that shares its name.
    pub fn merged(&self) -> Vec<&Plugin> {
        let mut merged: BTreeMap<&str, &Plugin> = BTreeMap::new();
        for plugins in [self.global.as_ref(), self.scoped.as_ref()]
            .into_iter()
            .flatten()
        {
            for plugin in plugins.list() {
                merged.insert(&plugin.id, plugin);
            }
        }

        merged.into_values().collect()
    }
}

pub fn routes() -> Router {
    Router::new()
        .get(list)
        .push(Router::with_path("{plugin_id}").get(one))
}

#[handler]
async fn list(
    plugins: ExtractPlugins,
    req: &mut Request,
    res: &mut Response,
) -> Result<(), StatusError> {
    if wants_events(req) {
        respond_events(res, plugins.stream(), events::resource_frame);

        return Ok(());
    }

    let base = mount(req.uri().path());
    let offset = req.query::<usize>("offset").unwrap_or(0);
    let limit = req.query::<usize>("limit").unwrap_or(usize::MAX);

    let document: Vec<PluginMetadata> = plugins
        .merged()
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|plugin| PluginMetadata::new(plugin, format!("{base}/{}", plugin.id)))
        .collect();
    res.render(Json(document));

    Ok(())
}

#[handler]
async fn one(
    plugins: ExtractPlugins,
    req: &mut Request,
) -> Result<Json<PluginMetadata>, StatusError> {
    let id = req
        .param::<String>("plugin_id")
        .ok_or_else(StatusError::not_found)?;
    // A single entry is served at its own address.
    let Some(plugin) = plugins.get(&id) else {
        return Err(StatusError::not_found());
    };

    Ok(Json(PluginMetadata::new(
        plugin,
        req.uri().path().to_owned(),
    )))
}

/// The mount path, without the trailing slash a nested route may leave behind.
fn mount(path: &str) -> &str {
    path.trim_end_matches('/')
}

