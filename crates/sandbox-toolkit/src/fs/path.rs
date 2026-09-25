//! The workspace root bounds a resolved path, so a symlink or a `..` cannot
//! escape: the candidate is canonicalized before it is compared against the root.

use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::workspace::registry::Workspace;

#[derive(Debug, Error)]
pub(crate) enum PathError {
    #[error("resource not found: {0}")]
    NotFound(String),
    #[error("invalid path `{path}`: {reason}")]
    InvalidPath { path: String, reason: String },
    #[error("failed to resolve the path: {0}")]
    Io(#[from] std::io::Error),
}

pub(crate) async fn resolve(
    workspace: Option<&Workspace>,
    path: &str,
) -> Result<PathBuf, PathError> {
    match workspace {
        Some(workspace) => {
            let relative = Path::new(path);
            if relative.is_absolute() {
                return Err(invalid(path, "path must be relative in workspace mode"));
            }

            let root = workspace.root();
            let target = canonicalize(root.join(relative), path).await?;
            if !target.starts_with(root) {
                return Err(invalid(path, "path escapes the workspace"));
            }

            Ok(target)
        }
        None => {
            if !Path::new(path).is_absolute() {
                return Err(invalid(path, "path must be absolute in direct mode"));
            }

            canonicalize(PathBuf::from(path), path).await
        }
    }
}

async fn canonicalize(path: PathBuf, address: &str) -> Result<PathBuf, PathError> {
    tokio::fs::canonicalize(&path).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            PathError::NotFound(address.to_owned())
        } else {
            PathError::Io(error)
        }
    })
}

fn invalid(path: &str, reason: &str) -> PathError {
    PathError::InvalidPath {
        path: path.to_owned(),
        reason: reason.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::model::WorkspaceProperties;
    use crate::workspace::registry::WorkspaceRegistry;
    use crate::workspace::registry::test_support::TempDir;

    async fn registered(dir: &TempDir) -> Workspace {
        let registry = WorkspaceRegistry::default();
        registry
            .register("docs", &dir.root(), WorkspaceProperties::default())
            .await
            .unwrap();

        registry.workspace("docs").unwrap()
    }

    #[tokio::test]
    async fn resolves_an_absolute_path_in_direct_mode() {
        let dir = TempDir::new();
        std::fs::write(dir.path().join("note.txt"), "hi").unwrap();

        let resolved = resolve(None, &dir.path().join("note.txt").display().to_string())
            .await
            .unwrap();

        assert_eq!(
            resolved,
            std::fs::canonicalize(dir.path().join("note.txt")).unwrap()
        );
    }

    #[tokio::test]
    async fn rejects_a_relative_path_in_direct_mode() {
        let error = resolve(None, "relative.txt").await.unwrap_err();

        assert!(matches!(error, PathError::InvalidPath { .. }));
    }

    #[tokio::test]
    async fn resolves_a_relative_path_inside_the_workspace() {
        let dir = TempDir::new();
        std::fs::write(dir.path().join("note.txt"), "hi").unwrap();
        let workspace = registered(&dir).await;

        let resolved = resolve(Some(&workspace), "note.txt").await.unwrap();

        assert_eq!(
            resolved,
            std::fs::canonicalize(dir.path().join("note.txt")).unwrap()
        );
    }

    #[tokio::test]
    async fn rejects_an_absolute_path_in_workspace_mode() {
        let dir = TempDir::new();
        std::fs::write(dir.path().join("note.txt"), "hi").unwrap();
        let workspace = registered(&dir).await;

        let error = resolve(
            Some(&workspace),
            &dir.path().join("note.txt").display().to_string(),
        )
        .await
        .unwrap_err();

        assert!(matches!(error, PathError::InvalidPath { .. }));
    }

    #[tokio::test]
    async fn rejects_a_path_that_escapes_through_a_parent() {
        let dir = TempDir::new();
        let outside = TempDir::new();
        std::fs::write(outside.path().join("secret.txt"), "secret").unwrap();
        let workspace = registered(&dir).await;

        let sibling = outside.path().file_name().unwrap().to_string_lossy();
        let error = resolve(Some(&workspace), &format!("../{sibling}/secret.txt"))
            .await
            .unwrap_err();

        assert!(matches!(error, PathError::InvalidPath { .. }));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rejects_a_symlink_that_escapes() {
        let dir = TempDir::new();
        let outside = TempDir::new();
        std::fs::write(outside.path().join("secret.txt"), "secret").unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret.txt"), dir.path().join("link"))
            .unwrap();
        let workspace = registered(&dir).await;

        let error = resolve(Some(&workspace), "link").await.unwrap_err();

        assert!(matches!(error, PathError::InvalidPath { .. }));
    }

    #[tokio::test]
    async fn reports_a_missing_target() {
        let dir = TempDir::new();

        let error = resolve(None, &dir.path().join("missing").display().to_string())
            .await
            .unwrap_err();

        assert!(matches!(error, PathError::NotFound(_)));
    }
}
