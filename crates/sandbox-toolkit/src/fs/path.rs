//! URL-to-path decoding. An address is percent-decoded once, verbatim.

use std::path::{Path, PathBuf};

use percent_encoding::percent_decode_str;

use crate::workspace::registry::{Workspace, WorkspaceError, WorkspaceRegistry};

/// Decode a workspace-relative address. The empty address is the root.
pub(crate) fn decode_relative(raw: &str) -> PathBuf {
    PathBuf::from(decode(raw))
}

/// Decode a remote absolute address. `raw` is the `/fs/` suffix without its
/// leading separator, so the empty address is the filesystem root.
pub(crate) fn decode_absolute(raw: &str) -> PathBuf {
    let decoded = decode(raw);
    if decoded.is_empty() {
        PathBuf::from("/")
    } else {
        Path::new("/").join(decoded)
    }
}

fn decode(raw: &str) -> String {
    percent_decode_str(raw).decode_utf8_lossy().into_owned()
}

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

    #[test]
    fn decodes_relative_addresses() {
        assert_eq!(decode_relative(""), PathBuf::new());
        assert_eq!(decode_relative("a/b.txt"), PathBuf::from("a/b.txt"));
        assert_eq!(
            decode_relative("a%20b/c"),
            PathBuf::from("a b/c"),
            "a percent escape decodes once"
        );
    }

    #[test]
    fn decodes_absolute_addresses() {
        assert_eq!(decode_absolute(""), PathBuf::from("/"));
        assert_eq!(
            decode_absolute("srv/project/a.txt"),
            PathBuf::from("/srv/project/a.txt")
        );
    }

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

        let absolute_path = dir.path().join("notes/todo.txt");
        let absolute = TargetFile::Absolute(absolute_path.clone());
        assert_eq!(absolute.address(), absolute_path.display().to_string());
        assert_eq!(absolute.path(), absolute_path);

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
