//! The workspace root bounds a resolved path, so a symlink or a `..` cannot
//! escape: the candidate is canonicalized before it is compared against the root.

use std::path::{Component, Path, PathBuf};

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
                return Err(invalid(path, "path escapes workspace"));
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

/// Resolve a path to its server-absolute location without following a final
/// symlink, so the entry itself can be inspected. The parent must exist and stay
/// inside the boundary; the final component may be missing, which is what makes
/// this usable for creations and deletions.
pub(crate) async fn resolve_entry(
    workspace: Option<&Workspace>,
    path: &str,
) -> Result<PathBuf, PathError> {
    let (root, relative) = match workspace {
        Some(workspace) => {
            let relative = Path::new(path);
            if relative.is_absolute() {
                return Err(invalid(path, "path must be relative in workspace mode"));
            }

            (Some(workspace.root().to_path_buf()), relative.to_path_buf())
        }
        None => {
            let absolute = Path::new(path);
            if !absolute.is_absolute() {
                return Err(invalid(path, "path must be absolute in direct mode"));
            }

            (None, absolute.to_path_buf())
        }
    };

    // A trailing `.`/`..` or a root has no final component to preserve; fall back
    // to the following resolution so normalization and boundary checks still run.
    let Some(name) = relative.file_name() else {
        return resolve(workspace, path).await;
    };

    let parent = relative.parent().unwrap_or_else(|| Path::new(""));
    let parent = match &root {
        Some(root) => root.join(parent),
        None => parent.to_path_buf(),
    };
    let parent = canonicalize(parent, path).await?;

    if let Some(root) = &root
        && !parent.starts_with(root)
    {
        return Err(invalid(path, "path escapes workspace"));
    }

    Ok(parent.join(name))
}

/// Resolve a path for creation, where the final component and any missing
/// ancestors do not exist yet. With no canonical target to compare, the
/// workspace boundary is checked lexically.
pub(crate) async fn resolve_new(
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
            let target = normalize(&root.join(relative));
            if !target.starts_with(root) {
                return Err(invalid(path, "path escapes workspace"));
            }

            Ok(target)
        }
        None => {
            if !Path::new(path).is_absolute() {
                return Err(invalid(path, "path must be absolute in direct mode"));
            }

            Ok(normalize(Path::new(path)))
        }
    }
}

/// Lexically normalize a path, resolving `.` and `..` without touching the
/// filesystem, so a symlink target can be checked before it exists.
pub(crate) fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => {}
            other => normalized.push(other.as_os_str()),
        }
    }

    normalized
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
