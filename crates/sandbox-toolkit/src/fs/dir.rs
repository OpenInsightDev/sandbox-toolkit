use std::path::{Path, PathBuf};
use std::time::SystemTime;

use thiserror::Error;

use super::file::{self, FileError};
use super::meta::{etag, timestamp};
use super::model::{
    CreateDirectoryRequest, DirectoryResponse, ListRequest, ResourceEntry, ResourceKind,
    ResourceMetadata,
};
use crate::path::{self, PathError};
use crate::workspace::registry::Workspace;

/// Entries a listing returns when the request names no `limit`, and the ceiling
/// an explicit `limit` is clamped to.
pub(crate) const SERVER_LIMIT: u64 = 1000;

#[derive(Debug, Error)]
pub(crate) enum DirectoryError {
    #[error("target is not a directory: {0}")]
    NotDirectory(String),
    #[error("resource already exists: {0}")]
    AlreadyExists(String),
    #[error("parent directory does not exist: {0}")]
    ParentNotFound(String),
    #[error("invalid glob pattern: {0}")]
    InvalidPattern(String),
    #[error(transparent)]
    Path(#[from] PathError),
    #[error(transparent)]
    File(#[from] FileError),
    #[error("failed to access the directory: {0}")]
    Io(#[from] std::io::Error),
}

pub(crate) async fn read_directory(
    workspace: Option<&Workspace>,
    request: &ListRequest,
) -> Result<DirectoryResponse, DirectoryError> {
    let root = checked_target(workspace, &request.path).await?;
    let limit = window(request.limit);
    let mut entries = Vec::new();
    let mut truncated = false;
    let mut visited = 0;

    let mut read_dir = tokio::fs::read_dir(&root).await?;
    while let Some(entry) = read_dir.next_entry().await? {
        if visited < request.offset.unwrap_or(0) {
            visited += 1;
            continue;
        }
        if entries.len() == limit {
            // One more entry sits past the window, so the response is short.
            truncated = true;
            break;
        }
        entries.push(resource_entry(&entry, &root, &request.path).await?);
    }

    Ok(DirectoryResponse { entries, truncated })
}

/// Symbolic links are reported as entries but never followed, so a
/// self-referential link cannot make the walk diverge.
pub(crate) async fn read_directory_recursive(
    workspace: Option<&Workspace>,
    request: &ListRequest,
) -> Result<DirectoryResponse, DirectoryError> {
    let root = checked_target(workspace, &request.path).await?;
    let limit = window(request.limit);
    let mut entries = Vec::new();
    let mut truncated = false;
    let mut visited = 0;

    let mut pending = vec![root.clone()];
    'walk: while let Some(directory) = pending.pop() {
        let mut read_dir = tokio::fs::read_dir(&directory).await?;
        while let Some(entry) = read_dir.next_entry().await? {
            let child = resource_entry(&entry, &root, &request.path).await?;
            // A directory is queued before the window check so a skipped subtree
            // is still walked; `resource_entry` classifies from `symlink_metadata`,
            // so a symlink is never a directory and is only listed.
            if child.kind == ResourceKind::Directory {
                pending.push(entry.path());
            }
            if visited < request.offset.unwrap_or(0) {
                visited += 1;
                continue;
            }
            if entries.len() == limit {
                truncated = true;
                break 'walk;
            }
            entries.push(child);
        }
    }

    Ok(DirectoryResponse { entries, truncated })
}

pub(crate) async fn create_directory(
    workspace: Option<&Workspace>,
    request: &CreateDirectoryRequest,
) -> Result<ResourceMetadata, DirectoryError> {
    let entry = path::resolve_new(workspace, &request.path).await?;
    ensure_absent(&entry).await?;

    if request.recursive == Some(true) {
        tokio::fs::create_dir_all(&entry)
            .await
            .map_err(|error| create_error(&entry, error))?;
    } else {
        let parent = entry
            .parent()
            .ok_or_else(|| DirectoryError::ParentNotFound(entry.display().to_string()))?;
        match tokio::fs::metadata(parent).await {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => {
                return Err(DirectoryError::ParentNotFound(parent.display().to_string()));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(DirectoryError::ParentNotFound(parent.display().to_string()));
            }
            Err(error) => return Err(error.into()),
        }

        tokio::fs::create_dir(&entry)
            .await
            .map_err(|error| create_error(&entry, error))?;
    }

    let metadata = tokio::fs::symlink_metadata(&entry).await?;

    Ok(file::describe(&entry, &request.path, &metadata).await?)
}

/// `create_dir_all` would accept an existing directory, so the target is checked
/// first to keep the conflict semantics of both creation modes.
async fn ensure_absent(path: &Path) -> Result<(), DirectoryError> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(_) => Err(DirectoryError::AlreadyExists(path.display().to_string())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// `AlreadyExists` covers the race with the existence check above, and
/// `NotADirectory` a component that exists as a file, the same conflict as a
/// missing parent.
fn create_error(path: &Path, error: std::io::Error) -> DirectoryError {
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

pub(crate) async fn checked_target(
    workspace: Option<&Workspace>,
    address: &str,
) -> Result<PathBuf, DirectoryError> {
    let resolved = path::resolve(workspace, address).await?;
    let metadata = tokio::fs::metadata(&resolved).await?;
    if !metadata.is_dir() {
        return Err(DirectoryError::NotDirectory(address.to_owned()));
    }

    Ok(resolved)
}

pub(crate) async fn resource_entry(
    entry: &tokio::fs::DirEntry,
    root: &Path,
    address_root: &str,
) -> Result<ResourceEntry, DirectoryError> {
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

    let relative = path.strip_prefix(root).unwrap_or(&path);

    Ok(ResourceEntry {
        name: entry.file_name().to_string_lossy().into_owned(),
        path: entry_address(address_root, relative),
        kind,
        // Only a file has content, matching how `metadata` reports size.
        size: if metadata.is_file() {
            metadata.len()
        } else {
            0
        },
        etag: etag(&metadata),
        modified_at: timestamp(metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH)),
    })
}

/// Address the entry in the request's mode: join the request path with the
/// entry's path relative to the listed root, so a recursive listing keeps its
/// directory prefix.
fn entry_address(address_root: &str, relative: &Path) -> String {
    Path::new(address_root)
        .join(relative)
        .to_string_lossy()
        .into_owned()
}

fn window(limit: Option<u64>) -> usize {
    limit.unwrap_or(SERVER_LIMIT).min(SERVER_LIMIT) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::model::Depth;
    use crate::workspace::registry::test_support::TempDir;

    fn list(path: &str) -> ListRequest {
        ListRequest {
            path: path.to_owned(),
            depth: None,
            offset: None,
            limit: None,
        }
    }

    fn create(path: &str, recursive: bool) -> CreateDirectoryRequest {
        CreateDirectoryRequest {
            path: path.to_owned(),
            recursive: Some(recursive),
        }
    }

    fn names(entries: &[ResourceEntry]) -> Vec<String> {
        let mut names: Vec<String> = entries.iter().map(|entry| entry.name.clone()).collect();
        names.sort_unstable();
        names
    }

    fn address(dir: &TempDir, relative: &str) -> String {
        dir.path().join(relative).display().to_string()
    }

    #[tokio::test]
    async fn reads_direct_children() {
        let dir = TempDir::new();
        std::fs::write(dir.path().join("z.txt"), "z").unwrap();
        std::fs::create_dir(dir.path().join("a")).unwrap();

        let response = read_directory(None, &list(&address(&dir, "")))
            .await
            .unwrap();

        assert_eq!(names(&response.entries), ["a", "z.txt"]);
        assert!(!response.truncated);
    }

    #[tokio::test]
    async fn recursive_read_walks_the_subtree_without_following_links() {
        let dir = TempDir::new();
        std::fs::create_dir_all(dir.path().join("a/b")).unwrap();
        std::fs::write(dir.path().join("a/b/c.txt"), "c").unwrap();
        std::os::unix::fs::symlink(dir.path(), dir.path().join("loop")).unwrap();

        let request = ListRequest {
            depth: Some(Depth::Infinity),
            ..list(&address(&dir, ""))
        };
        let response = read_directory_recursive(None, &request).await.unwrap();

        assert_eq!(names(&response.entries), ["a", "b", "c.txt", "loop"]);
    }

    #[tokio::test]
    async fn windows_a_listing() {
        let dir = TempDir::new();
        for name in ["a.txt", "b.txt", "c.txt"] {
            std::fs::write(dir.path().join(name), name).unwrap();
        }

        let request = ListRequest {
            limit: Some(2),
            ..list(&address(&dir, ""))
        };
        let response = read_directory(None, &request).await.unwrap();
        assert_eq!(response.entries.len(), 2);
        assert!(response.truncated);
    }

    #[tokio::test]
    async fn creates_a_single_directory_and_reports_an_existing_one() {
        let dir = TempDir::new();
        let path = address(&dir, "child");

        let metadata = create_directory(None, &create(&path, false)).await.unwrap();
        assert_eq!(metadata.kind, ResourceKind::Directory);
        assert!(dir.path().join("child").is_dir());

        assert!(matches!(
            create_directory(None, &create(&path, false)).await,
            Err(DirectoryError::AlreadyExists(_))
        ));
    }

    #[tokio::test]
    async fn recursive_create_builds_missing_parents() {
        let dir = TempDir::new();
        let path = address(&dir, "a/b/c");

        create_directory(None, &create(&path, true)).await.unwrap();

        assert!(dir.path().join("a/b/c").is_dir());
    }

    #[tokio::test]
    async fn a_single_create_reports_a_missing_parent() {
        let dir = TempDir::new();
        let path = address(&dir, "ghost/child");

        assert!(matches!(
            create_directory(None, &create(&path, false)).await,
            Err(DirectoryError::ParentNotFound(_))
        ));
    }
}
