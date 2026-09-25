//! Registration is the only place a remote absolute path is accepted;
//! [`WorkspaceRegistry::resolve`] is how the rest of the crate learns it.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{LazyLock, RwLock, RwLockReadGuard, RwLockWriteGuard},
};

use regex::Regex;
use thiserror::Error;

use super::model::{WorkspaceAccess, WorkspaceHandle, WorkspaceProperties};

const ENVIRONMENT_PREFIX: &str = "WORKSPACE_";

#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum WorkspaceError {
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
}

/// The value is the workspace root, so a command references its root through the
/// variable instead of carrying the remote absolute path itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkspaceEnvironment {
    pub(crate) name: String,
    pub(crate) value: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Workspace {
    id: String,
    root: PathBuf,
    properties: WorkspaceProperties,
}

impl Workspace {
    fn new(id: impl Into<String>, root: PathBuf, properties: WorkspaceProperties) -> Self {
        Self {
            id: id.into(),
            root,
            properties,
        }
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn access(&self) -> WorkspaceAccess {
        self.properties.access
    }

    fn handle(&self) -> WorkspaceHandle {
        WorkspaceHandle::new(self.id.clone(), self.properties)
    }

    fn require_writable(&self) -> Result<(), WorkspaceError> {
        match self.access() {
            WorkspaceAccess::ReadWrite => Ok(()),
            WorkspaceAccess::ReadOnly => Err(WorkspaceError::ReadOnly {
                id: self.id.clone(),
            }),
        }
    }
}

/// A path already validated as normalized: percent-decoded once, with no `.`,
/// `..`, NUL or ambiguous encoding. Resolution helpers may therefore join it
/// without normalizing again; only the workspace boundary still needs checking
/// before filesystem access.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TargetFile {
    Workspace {
        workspace: Workspace,
        relative_path: PathBuf,
    },
    Absolute(PathBuf),
}

impl TargetFile {
    pub(crate) fn path(&self) -> PathBuf {
        match self {
            Self::Workspace {
                workspace,
                relative_path,
            } => workspace.root().join(relative_path),
            Self::Absolute(path) => path.clone(),
        }
    }

    pub(crate) fn address(&self) -> String {
        match self {
            Self::Workspace { relative_path, .. } => relative_path.display().to_string(),
            Self::Absolute(path) => path.display().to_string(),
        }
    }

    pub(crate) fn workspace_root(&self) -> Option<&Path> {
        match self {
            Self::Workspace { workspace, .. } => Some(workspace.root()),
            Self::Absolute(_) => None,
        }
    }

    /// Direct mode has no workspace properties to narrow its behavior, so it is
    /// never read-only.
    pub(crate) fn require_writable(&self) -> Result<(), WorkspaceError> {
        match self {
            Self::Workspace { workspace, .. } => workspace.require_writable(),
            Self::Absolute(_) => Ok(()),
        }
    }
}

/// Handlers never hold a workspace root, only the id they resolve it with.
#[derive(Debug, Default)]
pub(crate) struct WorkspaceRegistry {
    workspaces: RwLock<HashMap<String, Workspace>>,
}

impl WorkspaceRegistry {
    /// A taken id is rejected before the root is inspected, so a duplicate is
    /// always reported as such.
    ///
    /// The root must already exist as a directory: a workspace that resolves to a
    /// missing path would confine nothing. The properties are fixed here.
    pub(crate) async fn register(
        &self,
        id: &str,
        root: &str,
        properties: WorkspaceProperties,
    ) -> Result<WorkspaceHandle, WorkspaceError> {
        validate_id(id)?;
        if self.read().contains_key(id) {
            return Err(WorkspaceError::AlreadyExists { id: id.to_owned() });
        }

        let root = canonical_root(root).await?;

        // Re-check under the write lock: the read above cannot close the gap
        // opened by releasing it for the await.
        let mut workspaces = self.write();
        if workspaces.contains_key(id) {
            return Err(WorkspaceError::AlreadyExists { id: id.to_owned() });
        }

        let workspace = Workspace::new(id, root, properties);
        let handle = workspace.handle();
        workspaces.insert(id.to_owned(), workspace);

        Ok(handle)
    }

    pub(crate) fn list(&self) -> Vec<WorkspaceHandle> {
        let workspaces = self.read();
        let mut ids: Vec<&str> = workspaces.keys().map(String::as_str).collect();
        ids.sort_unstable();

        ids.into_iter().map(|id| workspaces[id].handle()).collect()
    }

    pub(crate) fn get(&self, id: &str) -> Result<WorkspaceHandle, WorkspaceError> {
        self.read()
            .get(id)
            .map(Workspace::handle)
            .ok_or_else(|| WorkspaceError::NotFound { id: id.to_owned() })
    }

    pub(crate) fn workspace(&self, id: &str) -> Result<Workspace, WorkspaceError> {
        self.read()
            .get(id)
            .cloned()
            .ok_or_else(|| WorkspaceError::NotFound { id: id.to_owned() })
    }

    #[cfg_attr(not(test), expect(dead_code, reason = "used by process handlers"))]
    pub(crate) fn resolve(&self, id: &str) -> Result<PathBuf, WorkspaceError> {
        self.read()
            .get(id)
            .map(|workspace| workspace.root().to_owned())
            .ok_or_else(|| WorkspaceError::NotFound { id: id.to_owned() })
    }

    pub(crate) fn target_file(
        &self,
        id: &str,
        relative_path: PathBuf,
    ) -> Result<TargetFile, WorkspaceError> {
        Ok(TargetFile::Workspace {
            workspace: self.workspace(id)?,
            relative_path,
        })
    }

    /// Hands a process the root as a variable, so a command never has to hardcode
    /// the remote absolute path.
    pub(crate) fn environment(&self, id: &str) -> Result<WorkspaceEnvironment, WorkspaceError> {
        let workspaces = self.read();
        let workspace = workspaces
            .get(id)
            .ok_or_else(|| WorkspaceError::NotFound { id: id.to_owned() })?;

        Ok(WorkspaceEnvironment {
            name: environment_variable_name(workspace.id()),
            value: workspace.root().to_owned(),
        })
    }

    pub(crate) fn remove(&self, id: &str) -> Result<(), WorkspaceError> {
        let mut workspaces = self.write();
        if workspaces.remove(id).is_some() {
            Ok(())
        } else {
            Err(WorkspaceError::NotFound { id: id.to_owned() })
        }
    }

    /// A read guard over the workspace map, tolerating poisoning.
    ///
    /// A panic while a guard is held cannot leave the map inconsistent: every
    /// mutation is a single insert or remove. Recovering the guard therefore
    /// keeps the registry usable after an unrelated handler panics.
    fn read(&self) -> RwLockReadGuard<'_, HashMap<String, Workspace>> {
        self.workspaces
            .read()
            .unwrap_or_else(|error| error.into_inner())
    }

    fn write(&self) -> RwLockWriteGuard<'_, HashMap<String, Workspace>> {
        self.workspaces
            .write()
            .unwrap_or_else(|error| error.into_inner())
    }
}

/// The shared resource-id rules: `[a-z0-9]+(-[a-z0-9]+)*`.
///
/// The charset excludes separators such as `/` and `.`, so an id is usable as a
/// URL path segment. `\A`/`\z` rather than `^`/`$`, because `$` would accept a
/// trailing newline.
static ID_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\A[a-z0-9]+(?:-[a-z0-9]+)*\z").expect("the pattern is valid"));

/// The shared validation for any id arriving from outside the process, so a
/// malformed id is rejected before it is treated as merely unknown.
pub(crate) fn validate_id(id: &str) -> Result<(), WorkspaceError> {
    if ID_PATTERN.is_match(id) {
        Ok(())
    } else {
        Err(WorkspaceError::InvalidId { id: id.to_owned() })
    }
}

/// Uppercasing and mapping `-` to `_` yields a valid POSIX name; ids exclude `_`,
/// so the mapping is reversible and two ids never share a name.
fn environment_variable_name(id: &str) -> String {
    let mut name = String::with_capacity(ENVIRONMENT_PREFIX.len() + id.len());
    name.push_str(ENVIRONMENT_PREFIX);
    name.extend(id.chars().map(|character| match character {
        '-' => '_',
        other => other.to_ascii_uppercase(),
    }));

    name
}

/// Canonicalizing yields a stable path the boundary checks can compare against.
async fn canonical_root(root: &str) -> Result<PathBuf, WorkspaceError> {
    let path = Path::new(root);

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

#[cfg(test)]
pub(crate) mod test_support {
    use std::{
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    };

    pub(crate) struct TempDir(PathBuf);

    impl TempDir {
        pub(crate) fn new() -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);

            let path = std::env::temp_dir().join(format!(
                "sandbox-toolkit-workspace-test-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed),
            ));
            std::fs::create_dir_all(&path).expect("creating the temp dir failed");

            Self(path)
        }

        pub(crate) fn path(&self) -> &Path {
            &self.0
        }

        pub(crate) fn root(&self) -> String {
            self.0.to_str().expect("temp path is not UTF-8").to_owned()
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Canonical form of a temp dir, matching what the registry stores.
    pub(crate) fn canonical(path: &Path) -> PathBuf {
        std::fs::canonicalize(path).expect("canonicalizing the temp dir failed")
    }
}

#[cfg(test)]
mod tests {
    use super::{
        test_support::{TempDir, canonical},
        *,
    };

    #[tokio::test]
    async fn registers_and_resolves_a_workspace() {
        let dir = TempDir::new();
        let registry = WorkspaceRegistry::default();

        let workspace = registry
            .register("docs", &dir.root(), WorkspaceProperties::default())
            .await
            .unwrap();

        assert_eq!(workspace.id(), "docs");
        let stored = registry.read().get("docs").unwrap().clone();
        assert_eq!(stored.id(), "docs");
        assert_eq!(stored.root(), canonical(dir.path()));
        assert_eq!(registry.resolve("docs").unwrap(), canonical(dir.path()));
    }

    #[tokio::test]
    async fn target_files_keep_their_addressing_mode() {
        let dir = TempDir::new();
        let registry = WorkspaceRegistry::default();
        registry
            .register("docs", &dir.root(), WorkspaceProperties::default())
            .await
            .unwrap();

        let workspace = registry.workspace("docs").unwrap();
        let relative_path = PathBuf::from("notes/todo.txt");
        let target = TargetFile::Workspace {
            workspace: workspace.clone(),
            relative_path: relative_path.clone(),
        };
        assert_eq!(
            target,
            TargetFile::Workspace {
                workspace,
                relative_path,
            }
        );

        let absolute_path = dir.path().join("notes/todo.txt");
        assert_eq!(
            TargetFile::Absolute(absolute_path.clone()),
            TargetFile::Absolute(absolute_path)
        );
        assert_eq!(
            registry.workspace("missing"),
            Err(WorkspaceError::NotFound {
                id: "missing".to_owned()
            })
        );
    }

    #[tokio::test]
    async fn get_reports_registered_and_unknown_ids() {
        let dir = TempDir::new();
        let registry = WorkspaceRegistry::default();
        registry
            .register("docs", &dir.root(), WorkspaceProperties::default())
            .await
            .unwrap();

        assert_eq!(registry.get("docs").unwrap().id(), "docs");
        assert_eq!(
            registry.get("missing"),
            Err(WorkspaceError::NotFound {
                id: "missing".to_owned()
            })
        );
    }

    #[tokio::test]
    async fn rejects_a_duplicate_id_without_replacing_the_root() {
        let first = TempDir::new();
        let second = TempDir::new();
        let registry = WorkspaceRegistry::default();
        registry
            .register("docs", &first.root(), WorkspaceProperties::default())
            .await
            .unwrap();

        let error = registry
            .register("docs", &second.root(), WorkspaceProperties::default())
            .await
            .unwrap_err();

        assert_eq!(
            error,
            WorkspaceError::AlreadyExists {
                id: "docs".to_owned()
            }
        );
        assert_eq!(registry.resolve("docs").unwrap(), canonical(first.path()));
    }

    #[tokio::test]
    async fn rejects_a_duplicate_id_even_when_the_root_is_invalid() {
        let dir = TempDir::new();
        let registry = WorkspaceRegistry::default();
        registry
            .register("docs", &dir.root(), WorkspaceProperties::default())
            .await
            .unwrap();

        let error = registry
            .register("docs", "relative/path", WorkspaceProperties::default())
            .await
            .unwrap_err();

        assert_eq!(
            error,
            WorkspaceError::AlreadyExists {
                id: "docs".to_owned()
            }
        );
        assert_eq!(registry.resolve("docs").unwrap(), canonical(dir.path()));
    }

    #[tokio::test]
    async fn rejects_ids_outside_the_shared_charset() {
        let dir = TempDir::new();
        let registry = WorkspaceRegistry::default();

        for id in [
            "", "Docs", "a_b", "a.b", "a/b", "-a", "a-", "a--b", "a b", "a\n",
        ] {
            assert!(
                matches!(
                    registry
                        .register(id, &dir.root(), WorkspaceProperties::default())
                        .await,
                    Err(WorkspaceError::InvalidId { .. })
                ),
                "id `{id}` was accepted"
            );
        }
    }

    #[tokio::test]
    async fn rejects_roots_that_are_not_absolute_existing_directories() {
        let dir = TempDir::new();
        let file = dir.path().join("file.txt");
        std::fs::write(&file, b"content").unwrap();
        let registry = WorkspaceRegistry::default();

        for root in [
            "relative/path",
            "/definitely/not/registered/anywhere",
            file.to_str().unwrap(),
        ] {
            assert!(
                matches!(
                    registry
                        .register("docs", root, WorkspaceProperties::default())
                        .await,
                    Err(WorkspaceError::InvalidRoot { .. })
                ),
                "root `{root}` was accepted"
            );
        }
    }

    #[tokio::test]
    async fn lists_workspaces_in_id_order() {
        let dir = TempDir::new();
        let registry = WorkspaceRegistry::default();
        registry
            .register("zeta", &dir.root(), WorkspaceProperties::default())
            .await
            .unwrap();
        registry
            .register("alpha", &dir.root(), WorkspaceProperties::default())
            .await
            .unwrap();
        registry
            .register("mu", &dir.root(), WorkspaceProperties::default())
            .await
            .unwrap();

        let ids: Vec<String> = registry
            .list()
            .into_iter()
            .map(|workspace| workspace.id().to_owned())
            .collect();

        assert_eq!(ids, ["alpha", "mu", "zeta"]);
    }

    #[tokio::test]
    async fn remove_unregisters_a_workspace_once() {
        let dir = TempDir::new();
        let registry = WorkspaceRegistry::default();
        registry
            .register("docs", &dir.root(), WorkspaceProperties::default())
            .await
            .unwrap();

        registry.remove("docs").unwrap();

        assert_eq!(
            registry.resolve("docs"),
            Err(WorkspaceError::NotFound {
                id: "docs".to_owned()
            })
        );
        assert_eq!(
            registry.remove("docs"),
            Err(WorkspaceError::NotFound {
                id: "docs".to_owned()
            })
        );
    }

    #[tokio::test]
    async fn exposes_the_workspace_root_as_an_environment_variable() {
        let dir = TempDir::new();
        let registry = WorkspaceRegistry::default();
        registry
            .register("my-project", &dir.root(), WorkspaceProperties::default())
            .await
            .unwrap();

        let environment = registry.environment("my-project").unwrap();

        assert_eq!(environment.name, "WORKSPACE_MY_PROJECT");
        assert_eq!(environment.value, canonical(dir.path()));
    }

    #[tokio::test]
    async fn environment_reports_unknown_ids() {
        let registry = WorkspaceRegistry::default();

        assert_eq!(
            registry.environment("missing"),
            Err(WorkspaceError::NotFound {
                id: "missing".to_owned()
            })
        );
    }

    #[tokio::test]
    async fn stores_the_requested_access_mode() {
        let dir = TempDir::new();
        let registry = WorkspaceRegistry::default();

        registry
            .register(
                "sealed",
                &dir.root(),
                WorkspaceProperties {
                    access: WorkspaceAccess::ReadOnly,
                },
            )
            .await
            .unwrap();

        assert_eq!(
            registry.workspace("sealed").unwrap().access(),
            WorkspaceAccess::ReadOnly
        );
        assert_eq!(
            serde_json::to_value(registry.workspace("sealed").unwrap().handle()).unwrap(),
            serde_json::json!({ "id": "sealed", "properties": { "access": "read-only" } })
        );
    }

    #[tokio::test]
    async fn defaults_the_access_mode_to_read_write() {
        let dir = TempDir::new();
        let registry = WorkspaceRegistry::default();

        registry
            .register("docs", &dir.root(), WorkspaceProperties::default())
            .await
            .unwrap();

        assert_eq!(
            registry.workspace("docs").unwrap().access(),
            WorkspaceAccess::ReadWrite
        );
    }

    #[tokio::test]
    async fn mutations_require_a_writable_workspace() {
        let dir = TempDir::new();
        let registry = WorkspaceRegistry::default();
        registry
            .register("docs", &dir.root(), WorkspaceProperties::default())
            .await
            .unwrap();
        registry
            .register(
                "sealed",
                &dir.root(),
                WorkspaceProperties {
                    access: WorkspaceAccess::ReadOnly,
                },
            )
            .await
            .unwrap();

        let writable = registry
            .target_file("docs", PathBuf::from("note.txt"))
            .unwrap();
        assert_eq!(writable.require_writable(), Ok(()));

        let sealed = registry
            .target_file("sealed", PathBuf::from("note.txt"))
            .unwrap();
        assert_eq!(
            sealed.require_writable(),
            Err(WorkspaceError::ReadOnly {
                id: "sealed".to_owned(),
            })
        );

        // Direct mode carries no workspace properties.
        assert_eq!(
            TargetFile::Absolute(dir.path().to_owned()).require_writable(),
            Ok(())
        );
    }

    #[test]
    fn derives_environment_variable_names_from_ids() {
        for (id, name) in [
            ("docs", "WORKSPACE_DOCS"),
            ("my-project", "WORKSPACE_MY_PROJECT"),
            ("a1", "WORKSPACE_A1"),
        ] {
            assert_eq!(environment_variable_name(id), name);
        }

        // `-` maps to `_` and ids cannot contain `_`, so the mapping is injective.
        assert_ne!(
            environment_variable_name("a-b"),
            environment_variable_name("ab")
        );
    }
}
