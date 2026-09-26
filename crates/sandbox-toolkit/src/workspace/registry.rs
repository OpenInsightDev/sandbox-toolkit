//! Registration is the only place a remote absolute path is accepted.

use std::{
    collections::HashMap,
    ffi::OsString,
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

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// The `(name, value)` pair a child process reads its root through, ready to
    /// merge into its environment. Ids exclude `_`, so mapping `-` to `_` in the
    /// name stays reversible.
    pub(crate) fn env(&self) -> (String, OsString) {
        let id = self.id();
        let mut name = String::with_capacity(ENVIRONMENT_PREFIX.len() + id.len());
        name.push_str(ENVIRONMENT_PREFIX);
        name.extend(id.chars().map(|char| match char {
            '-' => '_',
            other => other.to_ascii_uppercase(),
        }));

        (name, self.root.clone().into_os_string())
    }

    fn access(&self) -> WorkspaceAccess {
        self.properties.access
    }

    fn handle(&self) -> WorkspaceHandle {
        WorkspaceHandle::new(self.id.clone(), self.properties)
    }

    pub(crate) fn require_writable(&self) -> Result<(), WorkspaceError> {
        match self.access() {
            WorkspaceAccess::ReadWrite => Ok(()),
            WorkspaceAccess::ReadOnly => Err(WorkspaceError::ReadOnly {
                id: self.id.clone(),
            }),
        }
    }
}

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

    /// Every registered workspace's variable, injected into a child process
    /// regardless of which workspace, if any, the request addresses.
    pub(crate) fn env(&self) -> HashMap<String, OsString> {
        self.read().values().map(Workspace::env).collect()
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
    ID_PATTERN
        .is_match(id)
        .then_some(())
        .ok_or_else(|| WorkspaceError::InvalidId { id: id.to_owned() })
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
        assert_eq!(
            registry.workspace("missing"),
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
        assert_eq!(
            registry.workspace("docs").unwrap().root(),
            canonical(first.path())
        );
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
        assert_eq!(
            registry.workspace("docs").unwrap().root(),
            canonical(dir.path())
        );
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
            registry.workspace("docs"),
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

        let workspace = registry.workspace("my-project").unwrap();

        assert_eq!(
            workspace.env(),
            (
                "WORKSPACE_MY_PROJECT".to_owned(),
                canonical(dir.path()).into_os_string()
            )
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

    #[test]
    fn derives_environment_variable_names_from_ids() {
        let name_of = |id: &str| {
            Workspace::new(id, PathBuf::new(), WorkspaceProperties::default())
                .env()
                .0
        };

        for (id, name) in [
            ("docs", "WORKSPACE_DOCS"),
            ("my-project", "WORKSPACE_MY_PROJECT"),
            ("a1", "WORKSPACE_A1"),
        ] {
            assert_eq!(name_of(id), name);
        }

        // `-` maps to `_` and ids cannot contain `_`, so the mapping is injective.
        assert_ne!(name_of("a-b"), name_of("ab"));
    }
}
