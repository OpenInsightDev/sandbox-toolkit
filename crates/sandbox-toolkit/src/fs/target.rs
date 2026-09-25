use std::path::{Path, PathBuf};

use crate::workspace::registry::{Workspace, WorkspaceError, WorkspaceRegistry};

/// Already normalized, so resolution helpers may join it without normalizing
/// again; only the workspace boundary still needs checking before filesystem
/// access.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TargetFile {
    Workspace {
        workspace: Workspace,
        relative_path: PathBuf,
    },
    Absolute(PathBuf),
}

impl TargetFile {
    pub(crate) fn workspace(
        registry: &WorkspaceRegistry,
        id: &str,
        relative_path: PathBuf,
    ) -> Result<Self, WorkspaceError> {
        Ok(Self::Workspace {
            workspace: registry.workspace(id)?,
            relative_path,
        })
    }

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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::workspace::model::{WorkspaceAccess, WorkspaceProperties};
    use crate::workspace::registry::{
        WorkspaceRegistry,
        test_support::{TempDir, canonical},
    };

    #[tokio::test]
    async fn target_files_keep_their_addressing_mode() {
        let dir = TempDir::new();
        let registry = WorkspaceRegistry::default();
        registry
            .register("docs", &dir.root(), WorkspaceProperties::default())
            .await
            .unwrap();

        let workspace =
            TargetFile::workspace(&registry, "docs", PathBuf::from("notes/todo.txt")).unwrap();
        let root = canonical(dir.path());
        assert_eq!(workspace.address(), "notes/todo.txt");
        assert_eq!(workspace.path(), root.join("notes/todo.txt"));
        assert_eq!(workspace.workspace_root(), Some(root.as_path()));

        let absolute_path = dir.path().join("notes/todo.txt");
        let absolute = TargetFile::Absolute(absolute_path.clone());
        assert_eq!(absolute.address(), absolute_path.display().to_string());
        assert_eq!(absolute.path(), absolute_path);
        assert_eq!(absolute.workspace_root(), None);

        assert_eq!(
            TargetFile::workspace(&registry, "missing", PathBuf::new()),
            Err(WorkspaceError::NotFound {
                id: "missing".to_owned()
            })
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

        let writable = TargetFile::workspace(&registry, "docs", PathBuf::from("note.txt")).unwrap();
        assert_eq!(writable.require_writable(), Ok(()));

        let sealed = TargetFile::workspace(&registry, "sealed", PathBuf::from("note.txt")).unwrap();
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
}
