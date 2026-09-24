//! Directory operations shared by the HTTP and MCP file surfaces.

use std::time::UNIX_EPOCH;

use thiserror::Error;

use super::model::{DirectoryResponse, FileEntry, FileMetadata, FileType};
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
    let path = checked_target(target).await?;
    let mut directory = tokio::fs::read_dir(&path).await?;
    let mut entries = Vec::new();

    while let Some(entry) = directory.next_entry().await? {
        entries.push(file_entry(entry).await?);
    }
    entries.sort_by(|left, right| left.name.cmp(&right.name));

    Ok(DirectoryResponse {
        path: path.display().to_string(),
        entries,
    })
}

/// Create one directory, equivalent to WebDAV `MKCOL`.
///
/// The operation deliberately does not create missing parents and never replaces an
/// existing resource; callers can map those errors to `409 Conflict`.
pub(crate) async fn create_directory(target: &TargetFile) -> Result<FileMetadata, DirectoryError> {
    let path = target.path();
    ensure_within_workspace(target, &path).await?;

    match tokio::fs::symlink_metadata(&path).await {
        Ok(_) => return Err(DirectoryError::AlreadyExists(path.display().to_string())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }

    let parent = path
        .parent()
        .ok_or_else(|| DirectoryError::ParentNotFound(path.display().to_string()))?;
    if !tokio::fs::metadata(parent).await?.is_dir() {
        return Err(DirectoryError::ParentNotFound(parent.display().to_string()));
    }

    tokio::fs::create_dir(&path).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            DirectoryError::AlreadyExists(path.display().to_string())
        } else {
            DirectoryError::Io(error)
        }
    })?;

    metadata(&path).await
}

async fn checked_target(target: &TargetFile) -> Result<std::path::PathBuf, DirectoryError> {
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
    let candidate = match tokio::fs::canonicalize(path).await {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let parent = path
                .parent()
                .ok_or_else(|| DirectoryError::OutsideWorkspace(path.display().to_string()))?;
            tokio::fs::canonicalize(parent).await?.join(
                path.file_name()
                    .ok_or_else(|| DirectoryError::OutsideWorkspace(path.display().to_string()))?,
            )
        }
        Err(error) => return Err(error.into()),
    };
    if candidate.starts_with(&root) {
        Ok(())
    } else {
        Err(DirectoryError::OutsideWorkspace(path.display().to_string()))
    }
}

async fn file_entry(entry: tokio::fs::DirEntry) -> Result<FileEntry, DirectoryError> {
    let path = entry.path();
    let metadata = tokio::fs::symlink_metadata(&path).await?;
    let file_type = if metadata.is_dir() {
        FileType::Directory
    } else if metadata.is_file() {
        FileType::File
    } else if metadata.file_type().is_symlink() {
        FileType::Symlink
    } else {
        return Err(DirectoryError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "unsupported directory entry type",
        )));
    };

    Ok(FileEntry {
        name: entry.file_name().to_string_lossy().into_owned(),
        path: path.display().to_string(),
        file_type,
        size: metadata.is_file().then_some(metadata.len()),
        etag: etag(&metadata),
        modified_at: modified_at(&metadata),
    })
}

async fn metadata(path: &std::path::Path) -> Result<FileMetadata, DirectoryError> {
    let value = tokio::fs::symlink_metadata(path).await?;
    Ok(FileMetadata {
        path: path.display().to_string(),
        file_type: FileType::Directory,
        size: None,
        etag: etag(&value),
        modified_at: modified_at(&value),
    })
}

pub(super) fn etag(metadata: &std::fs::Metadata) -> String {
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("\"{}-{}\"", metadata.len(), modified)
}

fn modified_at(metadata: &std::fs::Metadata) -> String {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::registry::test_support::TempDir;

    #[tokio::test]
    async fn reads_sorted_direct_children() {
        let dir = TempDir::new();
        tokio::fs::write(dir.path().join("z.txt"), "z")
            .await
            .unwrap();
        tokio::fs::create_dir(dir.path().join("a")).await.unwrap();
        let target = TargetFile::Absolute(dir.path().to_owned());

        let response = read_directory(&target).await.unwrap();
        assert_eq!(response.entries[0].name, "a");
        assert_eq!(response.entries[1].name, "z.txt");
    }

    #[tokio::test]
    async fn creates_only_a_single_directory() {
        let dir = TempDir::new();
        let target = TargetFile::Absolute(dir.path().join("child"));

        let metadata = create_directory(&target).await.unwrap();
        assert!(matches!(metadata.file_type, FileType::Directory));
        assert!(dir.path().join("child").is_dir());
        assert!(matches!(
            create_directory(&target).await,
            Err(DirectoryError::AlreadyExists(_))
        ));
    }
}
