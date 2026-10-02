use notify::EventKind;
use sandbox_toolkit_utils::watch::Watch;
use thiserror::Error;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;

use crate::mcp;
use crate::path::AGENTS_DIR;
use crate::workspace::model::CreateWorkspaceRequest;

use super::model::Metadata;
use super::resources::Resources;

use std::collections::HashMap;
use std::ffi::OsStr;
use std::ffi::OsString;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

const ENV_PREFIX: &str = "WORKSPACE_";
pub const GLOBAL_WORKSPACE_ID: &str = "global";

#[derive(Debug, Error)]
pub enum WorkspaceError {
    #[error("invalid workspace id `{id}`")]
    InvalidId { id: String },
    #[error("invalid workspace root `{root}`: {reason}")]
    InvalidRoot { root: String, reason: String },
    #[error("workspace `{id}` already exists")]
    AlreadyExists { id: String },
    #[error("workspace `{id}` does not exist")]
    NotFound { id: String },
    #[error("workspace `{id}` is read-only")]
    ReadOnly { id: String },
    #[error(transparent)]
    Watch(#[from] sandbox_toolkit_utils::watch::Error),
}

impl Metadata {
    pub async fn new(
        CreateWorkspaceRequest { id, root, access }: CreateWorkspaceRequest,
    ) -> Result<Self, WorkspaceError> {
        Ok(Metadata {
            id,
            root: canonical_root(root).await?,
            access,
        })
    }

    pub async fn new_global() -> Result<Self, WorkspaceError> {
        let id = GLOBAL_WORKSPACE_ID.to_owned();
        let root = dirs::home_dir().ok_or_else(|| WorkspaceError::InvalidRoot {
            root: "~".to_owned(),
            reason: "home directory is not available".to_owned(),
        })?;
        let access = super::model::WorkspaceAccess::ReadWrite;

        Ok(Metadata { id, root, access })
    }

    /// The `(name, value)` pair a child process reads its root through, ready to
    /// merge into its env. Ids exclude `_`, so mapping `-` to `_` in the
    /// name stays reversible.
    pub fn env(&self) -> (String, OsString) {
        let mut name = String::with_capacity(ENV_PREFIX.len() + self.id.len());
        name.push_str(ENV_PREFIX);
        name.extend(self.id.chars().map(|char| match char {
            '-' => '_',
            other => other.to_ascii_uppercase(),
        }));

        (name, self.root.clone().into_os_string())
    }
}

pub struct Workspace {
    metadata: Metadata,
    resources: Arc<RwLock<Option<Resources>>>,
    watch: Watch,
    handle: JoinHandle<()>,
}

impl Workspace {
    pub async fn new(metadata: Metadata) -> Result<Self, WorkspaceError> {
        let watch = Watch::new(metadata.root.join(AGENTS_DIR)).await?;
        let mut events = watch.subscribe();

        let resources = Arc::new(RwLock::new(None));
        sync_resources(&metadata.root, &resources).await;

        let task_resources = Arc::clone(&resources);
        let root = metadata.root.clone();
        let handle = tokio::spawn(async move {
            while let Ok(Ok(event)) = events.recv().await {
                match event.kind {
                    EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => {
                        sync_resources(&root, &task_resources).await;
                    }
                    EventKind::Access(_) | EventKind::Any | EventKind::Other => {}
                }
            }
        });

        Ok(Self {
            metadata,
            resources,
            watch,
            handle,
        })
    }

    pub async fn new_global() -> Result<Self, WorkspaceError> {
        Self::new(Metadata::new_global().await?).await
    }

    /// Cloned out so callers can hold a handle independent of the workspace registry lock.
    pub async fn mcps(&self) -> Option<Arc<mcp::Runtime>> {
        self.resources
            .read()
            .await
            .as_ref()
            .map(|resources| Arc::clone(&resources.mcps))
    }
}

async fn sync_resources(root: &Path, resources: &RwLock<Option<Resources>>) {
    let present = tokio::fs::metadata(root.join(AGENTS_DIR))
        .await
        .map(|metadata| metadata.is_dir())
        .unwrap_or(false);

    let mut guard = resources.write().await;
    match (present, guard.is_some()) {
        (true, false) => match Resources::new(root).await {
            Ok(next) => *guard = Some(next),
            Err(error) => tracing::warn!(%error, "failed to load workspace resources"),
        },
        (false, true) => *guard = None,
        _ => {}
    }
}

pub struct Registry {
    workspaces: RwLock<HashMap<String, Arc<Workspace>>>,
}

impl Registry {
    /// The global workspace is registered up front, so a registry always
    /// resolves at least the `global` scope.
    pub async fn new() -> Result<Self, WorkspaceError> {
        let global = Workspace::new_global().await?;

        let workspaces = HashMap::from([(GLOBAL_WORKSPACE_ID.to_owned(), Arc::new(global))]);
        let workspaces = RwLock::new(workspaces);
        Ok(Self { workspaces })
    }

    pub async fn register(&self, workspace: Workspace) -> Result<(), WorkspaceError> {
        let id = workspace.metadata.id.to_owned();

        let mut workspaces = self.workspaces.write().await;
        if workspaces.contains_key(&id) {
            return Err(WorkspaceError::AlreadyExists { id });
        }

        workspaces.insert(id, Arc::new(workspace));

        Ok(())
    }

    /// Resolves a registered workspace by id, cloned out of the map so the
    /// caller can use it after the lock is released.
    pub async fn get(&self, id: &str) -> Option<Arc<Workspace>> {
        self.workspaces.read().await.get(id).cloned()
    }

    pub async fn list(&self) -> Vec<Metadata> {
        self.workspaces
            .read()
            .await
            .values()
            .map(|v| v.metadata.to_owned())
            .collect()
    }

    pub async fn envs(&self) -> Vec<(String, OsString)> {
        self.workspaces
            .read()
            .await
            .values()
            .map(|workspace| workspace.metadata.env())
            .collect()
    }
}

/// Canonicalizing yields a stable path the boundary checks can compare against.
async fn canonical_root(root: impl AsRef<OsStr>) -> Result<PathBuf, WorkspaceError> {
    let path = Path::new(&root);
    let root = root.as_ref().to_string_lossy().to_string();

    if !path.is_absolute() {
        return Err(WorkspaceError::InvalidRoot {
            root: root.to_owned(),
            reason: "must be an absolute path".to_owned(),
        });
    }

    let metadata = tokio::fs::metadata(path)
        .await
        .map_err(|_| WorkspaceError::InvalidRoot {
            root: root.to_owned(),
            reason: "does not exist or is not readable".to_owned(),
        })?;

    if !metadata.is_dir() {
        return Err(WorkspaceError::InvalidRoot {
            root: root.to_owned(),
            reason: "must be a directory".to_owned(),
        });
    }

    tokio::fs::canonicalize(path)
        .await
        .map_err(|error| WorkspaceError::InvalidRoot {
            root: root.to_owned(),
            reason: error.to_string(),
        })
}
