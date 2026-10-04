use std::collections::BTreeMap;
use std::path::PathBuf;

use agent_plugins::{Manifest, SpecVersion};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Map, Value};
use ts_rs::TS;

use super::discover::Plugin;

#[derive(Debug, Clone, Serialize, JsonSchema, TS)]
#[ts(export)]
pub struct PluginMetadata {
    pub id: String,
    pub root: PathBuf,
    /// Where the mount that answered serves this plugin.
    pub uri: String,
    /// The manifest, and nothing of the components it declares.
    pub manifest: PluginManifest,
}

impl PluginMetadata {
    /// `uri` is the entry's address at the mount that answered.
    pub fn new(plugin: &Plugin, uri: String) -> Self {
        Self {
            id: plugin.id.clone(),
            root: plugin.root.clone(),
            uri,
            manifest: PluginManifest::new(&plugin.manifest),
        }
    }
}

/// The manifest as written: the fields the specification defines, and only the
/// optional ones the document actually carried.
#[derive(Debug, Clone, Serialize, JsonSchema, TS)]
#[ts(export)]
pub struct PluginManifest {
    #[serde(rename = "$schema")]
    schema: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub author: Option<PluginAuthor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub homepage: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub repository: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub license: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub extensions: Option<BTreeMap<String, Map<String, Value>>>,
}

impl PluginManifest {
    pub fn new(manifest: &Manifest) -> Self {
        let author = manifest.author.as_ref().map(|author| PluginAuthor {
            name: author.name.clone(),
            email: author.email.clone(),
            url: author.url.clone(),
        });

        let extensions: BTreeMap<String, Map<String, Value>> = manifest
            .extensions
            .iter()
            .map(|(namespace, data)| (namespace.to_owned(), data.clone()))
            .collect();

        Self {
            // The manifest was selected by its `$schema`, so the version it
            // declared is the version that validated it.
            schema: schema_url(manifest.spec),
            name: manifest.name.as_str().to_owned(),
            version: manifest.version.clone(),
            description: manifest.description.clone(),
            author,
            homepage: manifest.homepage.clone(),
            repository: manifest.repository.clone(),
            license: manifest.license.clone(),
            keywords: manifest.keywords.clone(),
            extensions: (!extensions.is_empty()).then_some(extensions),
        }
    }
}

#[derive(Debug, Clone, Serialize, JsonSchema, TS)]
#[ts(export)]
pub struct PluginAuthor {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub url: Option<String>,
}

fn schema_url(spec: SpecVersion) -> String {
    format!("https://agent-plugins.org/schemas/{spec}/plugin.schema.json")
}
