//! Directory operations shared by the HTTP and MCP resource surfaces.

use thiserror::Error;

use super::meta::{MetadataError, etag, modified_at, read_metadata};
use super::model::{DirectoryResponse, ResourceEntry, ResourceKind, ResourceMetadata};
use crate::workspace::registry::TargetFile;

#[derive(Debug, Error)]
pub(crate) enum DirectoryError {
    #[error("resource not found: {0}")]
    NotFound(String),
    #[error("target is not a directory: {0}")]
    NotDirectory(String),
    #[error("resource already exists: {0}")]
    AlreadyExists(String),
    #[error("parent directory does not exist: {0}")]
    ParentNotFound(String),
    #[error("path escapes workspace: {0}")]
    OutsideWorkspace(String),
    #[error("failed to access directory: {0}")]
    Io(#[from] std::io::Error),
}

/// Read a directory and its direct children, equivalent to WebDAV `PROPFIND` with `Depth: 1`.
pub(crate) async fn read_directory(
    target: &TargetFile,
) -> Result<DirectoryResponse, DirectoryError> {
    read_directory_with_depth(target, false).await
}

/// Read a directory and every descendant, equivalent to WebDAV `PROPFIND` with
/// `Depth: infinity`.
///
/// Symbolic links are reported as entries but never followed, so a self-referential
/// link cannot make the walk diverge.
pub(crate) async fn read_directory_recursive(
    target: &TargetFile,
) -> Result<DirectoryResponse, DirectoryError> {
    read_directory_with_depth(target, true).await
}

/// Walk `target`, descending into subdirectories when `recursive`.
async fn read_directory_with_depth(
    target: &TargetFile,
    recursive: bool,
) -> Result<DirectoryResponse, DirectoryError> {
    let path = checked_target(target).await?;
    let mut entries = Vec::new();
    let mut pending = vec![path.clone()];

    while let Some(directory) = pending.pop() {
        let mut read_dir = tokio::fs::read_dir(&directory).await?;
        while let Some(entry) = read_dir.next_entry().await? {
            let (child, child_path) = resource_entry(entry).await?;
            // `resource_entry` classifies from `symlink_metadata`, so a symlink is
            // never `Directory` and is only listed, not traversed. Reusing the entry's
            // own path avoids rebuilding it from the display string.
            if recursive && child.kind == ResourceKind::Directory {
                pending.push(child_path);
            }
            entries.push(child);
        }
    }

    Ok(DirectoryResponse {
        path: path.display().to_string(),
        entries,
        // A directory read is never paginated, so nothing is withheld.
        truncated: false,
    })
}

/// Create one directory, equivalent to WebDAV `MKCOL`.
///
/// The operation deliberately does not create missing parents and never replaces an
/// existing resource; callers can map those errors to `409 Conflict`.
pub(crate) async fn create_directory(
    target: &TargetFile,
) -> Result<ResourceMetadata, DirectoryError> {
    create_directory_with_parents(target, false).await
}

/// Create `target` and every missing parent directory.
///
/// Unlike [`create_directory`], the parents are created on demand; an existing
/// target is still a conflict rather than a quiet success.
pub(crate) async fn create_directory_recursive(
    target: &TargetFile,
) -> Result<ResourceMetadata, DirectoryError> {
    create_directory_with_parents(target, true).await
}

/// Create `target`, descending into missing parents when `recursive`.
async fn create_directory_with_parents(
    target: &TargetFile,
    recursive: bool,
) -> Result<ResourceMetadata, DirectoryError> {
    let path = target.path();
    ensure_within_workspace(target, &path).await?;

    // `create_dir_all` would accept an existing directory, so the target is
    // checked first to keep the conflict semantics in both modes.
    match tokio::fs::symlink_metadata(&path).await {
        Ok(_) => return Err(DirectoryError::AlreadyExists(path.display().to_string())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }

    if recursive {
        tokio::fs::create_dir_all(&path)
            .await
            .map_err(|error| create_error(&path, error))?;
    } else {
        let parent = path
            .parent()
            .ok_or_else(|| DirectoryError::ParentNotFound(path.display().to_string()))?;
        match tokio::fs::metadata(parent).await {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => return Err(DirectoryError::ParentNotFound(parent.display().to_string())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(DirectoryError::ParentNotFound(parent.display().to_string()));
            }
            Err(error) => return Err(error.into()),
        }

        tokio::fs::create_dir(&path)
            .await
            .map_err(|error| create_error(&path, error))?;
    }

    Ok(read_metadata(target).await?)
}

/// Map a `create_dir*` failure onto the directory error vocabulary.
///
/// `AlreadyExists` covers the race with the existence check above, and
/// `NotADirectory` a component that exists as a file, which is the same `MKCOL`
/// conflict as a missing parent.
fn create_error(path: &std::path::Path, error: std::io::Error) -> DirectoryError {
    match error.kind() {
        std::io::ErrorKind::AlreadyExists => {
            DirectoryError::AlreadyExists(path.display().to_string())
        }
        std::io::ErrorKind::NotADirectory => {
            DirectoryError::ParentNotFound(path.display().to_string())
        }
        _ => DirectoryError::Io(error),
    }
}

pub(super) async fn checked_target(
    target: &TargetFile,
) -> Result<std::path::PathBuf, DirectoryError> {
    let path = target.path();
    let metadata = tokio::fs::metadata(&path).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            DirectoryError::NotFound(path.display().to_string())
        } else {
            DirectoryError::Io(error)
        }
    })?;
    if !metadata.is_dir() {
        return Err(DirectoryError::NotDirectory(path.display().to_string()));
    }
    ensure_within_workspace(target, &path).await?;
    Ok(path)
}

async fn ensure_within_workspace(
    target: &TargetFile,
    path: &std::path::Path,
) -> Result<(), DirectoryError> {
    let Some(root) = target.workspace_root() else {
        return Ok(());
    };
    let root = tokio::fs::canonicalize(root).await?;

    // Resolve the deepest existing ancestor, so a target whose parents are still
    // missing can be checked. The components past that ancestor are literal names
    // from a normalized path, so nothing below it can escape.
    let mut current = path;
    let candidate = loop {
        match tokio::fs::canonicalize(current).await {
            Ok(resolved) => break resolved,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                current = current
                    .parent()
                    .ok_or_else(|| DirectoryError::OutsideWorkspace(path.display().to_string()))?;
            }
            Err(error) => return Err(error.into()),
        }
    };

    if candidate.starts_with(&root) {
        Ok(())
    } else {
        Err(DirectoryError::OutsideWorkspace(path.display().to_string()))
    }
}

pub(super) async fn resource_entry(
    entry: tokio::fs::DirEntry,
) -> Result<(ResourceEntry, std::path::PathBuf), DirectoryError> {
    let path = entry.path();
    let metadata = tokio::fs::symlink_metadata(&path).await?;
    let kind = if metadata.is_dir() {
        ResourceKind::Directory
    } else if metadata.is_file() {
        ResourceKind::File
    } else if metadata.file_type().is_symlink() {
        ResourceKind::Symlink
    } else {
        return Err(DirectoryError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "unsupported directory entry type",
        )));
    };

    let entry = ResourceEntry {
        name: entry.file_name().to_string_lossy().into_owned(),
        path: path.display().to_string(),
        kind,
        size: metadata.is_file().then_some(metadata.len()),
        etag: etag(&metadata),
        modified_at: modified_at(&metadata),
    };

    Ok((entry, path))
}

impl From<MetadataError> for DirectoryError {
    fn from(error: MetadataError) -> Self {
        match error {
            MetadataError::NotFound(path) => Self::NotFound(path),
            MetadataError::OutsideWorkspace(path) => Self::OutsideWorkspace(path),
            MetadataError::Io(error) => Self::Io(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::registry::test_support::TempDir;

    #[tokio::test]
    async fn reads_direct_children() {
        let dir = TempDir::new();
        tokio::fs::write(dir.path().join("z.txt"), "z")
            .await
            .unwrap();
        tokio::fs::create_dir(dir.path().join("a")).await.unwrap();
        let target = TargetFile::Absolute(dir.path().to_owned());

        let response = read_directory(&target).await.unwrap();
        let mut names: Vec<&str> = response
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        names.sort_unstable();
        assert_eq!(names, ["a", "z.txt"]);
    }

    #[tokio::test]
    async fn recursive_read_walks_the_whole_subtree() {
        let dir = TempDir::new();
        tokio::fs::create_dir_all(dir.path().join("a/b"))
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("a/b/c.txt"), "c")
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("z.txt"), "z")
            .await
            .unwrap();
        let target = TargetFile::Absolute(dir.path().to_owned());

        let response = read_directory_recursive(&target).await.unwrap();
        let mut names: Vec<&str> = response
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        names.sort_unstable();
        assert_eq!(names, ["a", "b", "c.txt", "z.txt"]);
    }

    #[tokio::test]
    async fn recursive_read_does_not_follow_symlinks() {
        let dir = TempDir::new();
        tokio::fs::create_dir(dir.path().join("a")).await.unwrap();
        tokio::fs::symlink(dir.path(), dir.path().join("loop"))
            .await
            .unwrap();
        let target = TargetFile::Absolute(dir.path().to_owned());

        let response = read_directory_recursive(&target).await.unwrap();
        let mut names: Vec<&str> = response
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        names.sort_unstable();
        assert_eq!(names, ["a", "loop"]);
        let loop_entry = response
            .entries
            .iter()
            .find(|entry| entry.name == "loop")
            .unwrap();
        assert_eq!(loop_entry.kind, ResourceKind::Symlink);
    }

    #[tokio::test]
    async fn creates_only_a_single_directory() {
        let dir = TempDir::new();
        let target = TargetFile::Absolute(dir.path().join("child"));

        let metadata = create_directory(&target).await.unwrap();
        assert_eq!(metadata.kind, ResourceKind::Directory);
        assert!(dir.path().join("child").is_dir());
        assert!(matches!(
            create_directory(&target).await,
            Err(DirectoryError::AlreadyExists(_))
        ));
    }

    #[tokio::test]
    async fn recursive_create_builds_missing_parents() {
        let dir = TempDir::new();
        let target = TargetFile::Absolute(dir.path().join("a/b/c"));

        let metadata = create_directory_recursive(&target).await.unwrap();
        assert_eq!(metadata.kind, ResourceKind::Directory);
        assert!(dir.path().join("a/b/c").is_dir());
    }

    #[tokio::test]
    async fn recursive_create_still_reports_an_existing_target() {
        let dir = TempDir::new();
        tokio::fs::create_dir(dir.path().join("a")).await.unwrap();
        let target = TargetFile::Absolute(dir.path().join("a"));

        assert!(matches!(
            create_directory_recursive(&target).await,
            Err(DirectoryError::AlreadyExists(_))
        ));
    }
}
