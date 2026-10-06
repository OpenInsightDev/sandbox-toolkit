use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent_plugins::{
    DirSource, DirEntry, FileKind, LoadedPlugin, Manifest, McpDisabledReason, McpOutcome,
    PackagePath, Rejection, ServerEntry, Source,
};
use thiserror::Error;

use crate::events::{self, Digest};
use crate::path::{AGENTS_DIR, MCP_JSON, PLUGINS_DIR, PLUGIN_JSON, SKILL_MD};
use crate::skill::Skill;

/// One package found in a scope's `.agents/plugins`, already loaded by
/// `agent-plugins`.
pub struct Plugin {
    /// The plugin directory's name, which the specification fixes as the id.
    pub id: String,
    /// The plugin directory, canonical.
    pub root: PathBuf,
    /// The `plugin.json` it was loaded from, as content.
    pub digest: Digest,
    /// The package's `mcp.json` as content, [`events::ABSENT`] when it ships
    /// none.
    pub mcp_digest: Digest,
    pub manifest: Manifest,
    /// The plugin's skills, each id prefixed with the plugin id.
    pub skills: Vec<Skill>,
    /// The `mcp.json` entries that survived their own validation.
    pub servers: Vec<ServerEntry>,
}

/// A plugin package directory whose reads are remembered, so every source the
/// loader read has an identity without being read a second time.
struct Recorded {
    directory: DirSource,
    read: BTreeMap<String, Vec<u8>>,
}

impl Recorded {
    fn open(directory: &Path) -> io::Result<Self> {
        Ok(Self {
            directory: DirSource::open(directory)?,
            read: BTreeMap::new(),
        })
    }

    /// What `path` held when the loader read it.
    fn digest(&self, path: &str) -> Digest {
        match self.read.get(path) {
            Some(bytes) => events::digest(bytes),
            None => events::ABSENT,
        }
    }
}

impl Source for Recorded {
    type Error = io::Error;

    fn kind(&mut self, path: &PackagePath) -> Result<Option<FileKind>, io::Error> {
        self.directory.kind(path)
    }

    fn confined(&mut self, path: &PackagePath) -> Result<bool, io::Error> {
        self.directory.confined(path)
    }

    fn read(&mut self, path: &PackagePath) -> Result<Vec<u8>, io::Error> {
        let bytes = self.directory.read(path)?;
        self.read.insert(path.as_str().to_owned(), bytes.clone());

        Ok(bytes)
    }

    fn list(&mut self, path: &PackagePath) -> Result<Vec<DirEntry>, io::Error> {
        self.directory.list(path)
    }
}

impl Plugin {
    /// The extension namespaces the manifest declares.
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

    /// Reads the same directory again, as it is now.
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
    let mut source = Recorded::open(directory)?;
    let LoadedPlugin {
        manifest,
        skills,
        mcp,
        diagnostics,
        ..
    } = agent_plugins::load(&mut source).map_err(|rejection| Error::Rejected {
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
                digest: source.digest(&format!("{}/{SKILL_MD}", skill.path.as_str())),
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
        digest: source.digest(PLUGIN_JSON),
        mcp_digest: source.digest(MCP_JSON),
        manifest,
        skills,
        servers,
    })
}