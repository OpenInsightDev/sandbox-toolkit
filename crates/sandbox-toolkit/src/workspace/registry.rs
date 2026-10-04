use notify::EventKind;
use sandbox_toolkit_utils::watch::Watch;
use thiserror::Error;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;

use crate::binary::path;
use crate::path::AGENTS_DIR;
use crate::skill::{Skills, split_derived_workspace_id};
use crate::workspace::model::CreateWorkspaceRequest;

use super::model::Metadata;
use super::model::WorkspaceAccess;
use super::resources::Resources;

use std::collections::HashMap;
use std::ffi::OsStr;
use std::ffi::OsString;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

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
    #[error("workspace `{id}` is immutable")]
    Immutable { id: String },
    #[error("insufficient permission on workspace root `{root}`")]
    PermissionDenied { root: String },
    #[error("workspace `{id}` is read-only")]
    ReadOnly { id: String },
    #[error(transparent)]
    Watch(#[from] sandbox_toolkit_utils::watch::Error),
}

impl Metadata {
    pub async fn new(
        CreateWorkspaceRequest { id, root, access }: CreateWorkspaceRequest,
    ) -> Result<Self, WorkspaceError> {
        if !is_url_safe(&id) {
            return Err(WorkspaceError::InvalidId { id });
        }

        let root = canonical_root(root).await?;
        probe_access(&root).await?;

        Ok(Metadata { id, root, access })
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

    pub fn child_env(
        &self,
        bin: &Path,
        overrides: &HashMap<String, String>,
    ) -> Vec<(OsString, OsString)> {
        let mut env: HashMap<OsString, OsString> = std::env::vars_os().collect();
        env.insert("PATH".into(), path::for_subprocess(bin));
        let (name, root) = self.env();
        env.insert(name.into(), root);
        env.extend(
            overrides
                .iter()
                .map(|(name, value)| (name.into(), value.into())),
        );

        env.into_iter().collect()
    }
}

pub struct Workspace {
    metadata: RwLock<Metadata>,
    resources: Arc<RwLock<Option<Arc<Resources>>>>,
    watch: Watch,
    handle: JoinHandle<()>,
}

impl Workspace {
    pub async fn new(metadata: Metadata) -> Result<Self, WorkspaceError> {
        let watch = Watch::new(metadata.root.join(AGENTS_DIR)).await?;
        let mut events = watch.subscribe();

        let resources = Arc::new(RwLock::new(None));
        sync_resources(&metadata, &resources).await;

        let task_resources = Arc::clone(&resources);
        let task_metadata = metadata.clone();
        let handle = tokio::spawn(async move {
            while let Ok(Ok(event)) = events.recv().await {
                match event.kind {
                    EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => {
                        sync_resources(&task_metadata, &task_resources).await;
                    }
                    EventKind::Access(_) | EventKind::Any | EventKind::Other => {}
                }
            }
        });

        Ok(Self {
            metadata: RwLock::new(metadata),
            resources,
            watch,
            handle,
        })
    }

    pub async fn new_global() -> Result<Self, WorkspaceError> {
        Self::new(Metadata::new_global().await?).await
    }

    pub async fn metadata(&self) -> Metadata {
        self.metadata.read().await.clone()
    }

    /// `access` is declarative, so it changes without re-probing the root.
    pub async fn set_access(&self, access: WorkspaceAccess) {
        self.metadata.write().await.access = access;
    }

    /// Cloned out so callers can hold the resource set independent of the
    /// workspace registry lock.
    pub async fn resources(&self) -> Option<Arc<Resources>> {
        self.resources.read().await.clone()
    }
}

pub enum Resolved {
    Registered(Arc<Workspace>),
    /// The view a skill discovered in a scope derives.
    Derived(Metadata),
}

impl Resolved {
    pub async fn metadata(&self) -> Metadata {
        match self {
            Self::Registered(workspace) => workspace.metadata().await,
            Self::Derived(metadata) => metadata.clone(),
        }
    }

    /// The resource set the workspace holds; a derived workspace holds none.
    pub async fn resources(&self) -> Option<Arc<Resources>> {
        match self {
            Self::Registered(workspace) => workspace.resources().await,
            Self::Derived(_) => None,
        }
    }
}

async fn sync_resources(metadata: &Metadata, resources: &RwLock<Option<Arc<Resources>>>) {
    let present = tokio::fs::metadata(metadata.root.join(AGENTS_DIR))
        .await
        .map(|metadata| metadata.is_dir())
        .unwrap_or(false);

    let mut guard = resources.write().await;
    match (present, guard.is_some()) {
        (true, false) => match Resources::new(metadata).await {
            Ok(next) => *guard = Some(Arc::new(next)),
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
        let id = workspace.metadata().await.id;

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

    pub async fn resolve(&self, id: &str) -> Option<Resolved> {
        if let Some(workspace) = self.get(id).await {
            return Some(Resolved::Registered(workspace));
        }

        self.derive(id).await
    }

    /// The workspace `skill.{scope}.{skill_id}` derives. It resolves while its
    /// `{scope}` is `global` or a registered workspace, and `{skill_id}` is a
    /// skill that scope discovers right now.
    async fn derive(&self, id: &str) -> Option<Resolved> {
        let (scope_id, skill_id) = split_derived_workspace_id(id)?;
        let scope = self.get(scope_id).await?;
        let metadata = scope.metadata().await;

        let skill = Skills::new(&metadata.id, &metadata.root)
            .get(skill_id)
            .await?;

        Some(Resolved::Derived(Metadata {
            id: id.to_owned(),
            root: skill.root,
            access: metadata.access,
        }))
    }

    pub async fn list(&self) -> Vec<Metadata> {
        let workspaces = self.workspaces.read().await;
        let mut list = Vec::with_capacity(workspaces.len());
        for workspace in workspaces.values() {
            list.push(workspace.metadata().await);
        }
        list
    }

    pub async fn envs(&self) -> Vec<(String, OsString)> {
        let workspaces = self.workspaces.read().await;
        let mut envs = Vec::with_capacity(workspaces.len());
        for workspace in workspaces.values() {
            envs.push(workspace.metadata().await.env());
        }
        envs
    }

    pub async fn update_access(
        &self,
        id: &str,
        access: WorkspaceAccess,
    ) -> Result<Metadata, WorkspaceError> {
        let workspace = self.mutable(id).await?;
        workspace.set_access(access).await;
        Ok(workspace.metadata().await)
    }

    pub async fn remove(&self, id: &str) -> Result<(), WorkspaceError> {
        self.mutable(id).await?;

        self.workspaces.write().await.remove(id);
        Ok(())
    }

    /// Resolves a workspace the caller may mutate. Only a registered workspace
    /// is mutable: the preset `global` and a derived view answer `403`.
    async fn mutable(&self, id: &str) -> Result<Arc<Workspace>, WorkspaceError> {
        if id == GLOBAL_WORKSPACE_ID {
            return Err(WorkspaceError::Immutable { id: id.to_owned() });
        }

        match self.resolve(id).await {
            Some(Resolved::Registered(workspace)) => Ok(workspace),
            Some(Resolved::Derived(_)) => Err(WorkspaceError::Immutable { id: id.to_owned() }),
            None => Err(WorkspaceError::NotFound { id: id.to_owned() }),
        }
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

/// An `id` is captured verbatim from `/workspaces/{id}`, so only unreserved
/// ASCII is allowed, which the router reproduces without escaping.
fn is_url_safe(id: &str) -> bool {
    if id.is_empty() {
        return false;
    }

    id.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '~')
}

/// Probes read and write as the service process rather than reading mode bits,
/// which cannot see e.g. a read-only mount owned by the same uid.
async fn probe_access(root: &Path) -> Result<(), WorkspaceError> {
    let denied = || WorkspaceError::PermissionDenied {
        root: root.to_string_lossy().into_owned(),
    };

    let mut entries = tokio::fs::read_dir(root).await.map_err(|_| denied())?;
    entries.next_entry().await.map_err(|_| denied())?;

    let probe = root.join(probe_name());
    tokio::fs::write(&probe, []).await.map_err(|_| denied())?;
    tokio::fs::remove_file(&probe).await.map_err(|_| denied())?;

    Ok(())
}

fn probe_name() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let serial = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!(".sbxtkt-probe-{}-{serial}", std::process::id())
}
