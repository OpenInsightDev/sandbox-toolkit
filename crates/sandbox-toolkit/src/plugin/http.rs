use std::collections::BTreeMap;

use salvo::prelude::*;

use super::discover::{Plugin, Plugins};
use super::model::PluginMetadata;

pub struct ExtractPlugins {
    global: Option<Plugins>,
    scoped: Option<Plugins>,
}

impl ExtractPlugins {
    pub fn new(global: Option<Plugins>, scoped: Option<Plugins>) -> Self {
        Self { global, scoped }
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
async fn list(plugins: ExtractPlugins, req: &mut Request) -> Result<Json<Vec<PluginMetadata>>, StatusError> {
    let base = mount(req.uri().path());
    let offset = req.query::<usize>("offset").unwrap_or(0);
    let limit = req.query::<usize>("limit").unwrap_or(usize::MAX);

    let document = plugins
        .merged()
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|plugin| PluginMetadata::new(plugin, format!("{base}/{}", plugin.id)))
        .collect();

    Ok(Json(document))
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

