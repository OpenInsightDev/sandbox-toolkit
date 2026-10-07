use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent_plugins::{
    LoadedPlugin, Manifest, McpDisabledReason, McpOutcome, Rejection, ServerEntry,
};
use thiserror::Error;

use crate::path::{AGENTS_DIR, PLUGINS_DIR};
use crate::skill::Skill;

/// One package found in a scope's `.agents/plugins`, already loaded by
/// `agent-plugins`.
pub struct Plugin {
    /// The plugin directory's name, which the specification fixes as the id.
    pub id: String,
    /// The plugin directory, canonical.
    pub root: PathBuf,
    pub manifest: Manifest,
    /// The plugin's skills, each id prefixed with the plugin id.
    pub skills: Vec<Skill>,
    /// The `mcp.json` entries that survived their own validation.
    pub servers: Vec<ServerEntry>,
}

impl Plugin {
    pub fn namespaces(&self) -> impl Iterator<Item = &str> {
        self.manifest.extensions.iter().map(|(name, _)| name)
    }

    /// The directory a namespace's derived workspace is rooted at. A manifest
    /// may declare data alone, so the path stands whether or not it exists.
    pub fn namespace_root(&self, namespace: &str) -> PathBuf {
        let path = self.root.join(namespace);
        std::fs::canonicalize(&path).unwrap_or(path)
    }
}

/// The plugins a scope's `.agents/plugins` holds, in id order.
///
/// Discovery follows the spec's failure boundaries: a rejected manifest or a
/// disabled MCP component fails the whole set, while the non-fatal problems the
/// specification reports are ignored.
#[derive(Clone)]
pub struct Plugins {
    root: PathBuf,
    loaded: Arc<Vec<Plugin>>,
}

impl Plugins {
    /// Reads the directory as it is now.
    pub fn load(root: impl Into<PathBuf>) -> Result<Self, Error> {
        let root = root.into();
        let directory = root.join(AGENTS_DIR).join(PLUGINS_DIR);
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            // A scope that ships no plugins is not a failure: it simply has none.
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(Self {
                    root,
                    loaded: Arc::new(Vec::new()),
                });
            }
            Err(error) => return Err(error.into()),
        };

        let mut plugins = Vec::new();
        for entry in entries {
            let entry = entry?;
            // Only a directory is a package; anything else is not a plugin.
            if !entry.file_type()?.is_dir() {
                continue;
            }

            // The directory name is the id, so one that is not text cannot be one.
            let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            plugins.push(discover(&entry.path(), id)?);
        }
        plugins.sort_by(|left, right| left.id.cmp(&right.id));

        Ok(Self {
            root,
            loaded: Arc::new(plugins),
        })
    }

    pub fn rescan(&self) -> Result<Self, Error> {
        Self::load(self.root.clone())
    }

    pub fn list(&self) -> &[Plugin] {
        &self.loaded
    }

    pub fn get(&self, id: &str) -> Option<&Plugin> {
        self.list().iter().find(|plugin| plugin.id == id)
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Read(#[from] io::Error),
    #[error("the plugin at `{directory}` was rejected: {rejection}")]
    Rejected {
        directory: PathBuf,
        rejection: Rejection,
    },
    #[error("plugin `{id}` has its MCP component disabled: {reason}")]
    Disabled {
        id: String,
        reason: McpDisabledReason,
    },
}

fn discover(directory: &Path, id: String) -> Result<Plugin, Error> {
    let LoadedPlugin {
        manifest,
        skills,
        mcp,
        diagnostics,
        ..
    } = agent_plugins::load_dir(directory).map_err(|rejection| Error::Rejected {
        directory: directory.to_path_buf(),
        rejection,
    })?;

    // The specification reports these and carries on; they never fail a load.
    for diagnostic in &diagnostics {
        tracing::debug!(%diagnostic, "ignored a plugin problem");
    }

    let root = std::fs::canonicalize(directory)?;
    let skills = skills
        .iter()
        .map(|skill| {
            Ok(Skill {
                id: format!("{id}.{}", skill.meta.name),
                root: std::fs::canonicalize(skill.path.to_native(&root))?,
                meta: skill.meta.clone(),
                body: skill.body.source().to_owned(),
            })
        })
        .collect::<Result<Vec<_>, io::Error>>()?;

    let servers = match mcp {
        McpOutcome::Configured(config) => config.servers,
        McpOutcome::Disabled(reason) => return Err(Error::Disabled { id, reason }),
        // No `mcp.json`, and any outcome a later specification adds that this
        // build cannot serve, leaves the component without entries.
        _ => Vec::new(),
    };

    Ok(Plugin {
        id,
        root,
        manifest,
        skills,
        servers,
    })
}