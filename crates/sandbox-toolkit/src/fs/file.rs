use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use thiserror::Error;

use super::model::{ContentRequest, ContentResponse};
use crate::workspace::registry::Workspace;

#[derive(Debug, Error)]
pub(crate) enum ReadError {
    #[error("resource not found: {0}")]
    NotFound(String),
    #[error("target is not a file: {0}")]
    NotAFile(String),
    #[error("invalid path `{path}`: {reason}")]
    InvalidPath { path: String, reason: String },
    #[error("file content is not valid UTF-8; use `QUERY ?type=stream` for binary data")]
    NotUtf8,
    #[error("failed to read the file: {0}")]
    Io(#[from] std::io::Error),
}

pub(crate) async fn read(
    workspace: Option<&Workspace>,
    request: ContentRequest,
) -> Result<ContentResponse, ReadError> {
    let resolved = resolve(workspace, &request.path).await?;
    let metadata = tokio::fs::metadata(&resolved).await?;
    if !metadata.is_file() {
        return Err(ReadError::NotAFile(request.path));
    }

    let bytes = tokio::fs::read(&resolved).await?;
    let size = bytes.len() as u64;
    let content = String::from_utf8(bytes).map_err(|_| ReadError::NotUtf8)?;

    Ok(ContentResponse {
        path: request.path,
        content,
        size,
        etag: etag(&metadata),
    })
}

/// Resolving first is what makes the boundary check meaningful: a symlink or a
/// `..` that lands outside the workspace is caught only after it is resolved.
async fn resolve(workspace: Option<&Workspace>, path: &str) -> Result<PathBuf, ReadError> {
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

async fn canonicalize(path: PathBuf, address: &str) -> Result<PathBuf, ReadError> {
    tokio::fs::canonicalize(&path).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ReadError::NotFound(address.to_owned())
        } else {
            ReadError::Io(error)
        }
    })
}

fn invalid(path: &str, reason: &str) -> ReadError {
    ReadError::InvalidPath {
        path: path.to_owned(),
        reason: reason.to_owned(),
    }
}

fn etag(metadata: &std::fs::Metadata) -> String {
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();

    format!("\"{}-{}\"", metadata.len(), modified)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::model::WorkspaceProperties;
    use crate::workspace::registry::WorkspaceRegistry;
    use crate::workspace::registry::test_support::TempDir;

    fn request(path: impl Into<String>) -> ContentRequest {
        ContentRequest { path: path.into() }
    }

    async fn registered(dir: &TempDir) -> Workspace {
        let registry = WorkspaceRegistry::default();
        registry
            .register("docs", &dir.root(), WorkspaceProperties::default())
            .await
            .unwrap();

        registry.workspace("docs").unwrap()
    }

    #[tokio::test]
    async fn reads_utf8_content_in_direct_mode() {
        let dir = TempDir::new();
        std::fs::write(dir.path().join("note.txt"), "hello").unwrap();

        let response = read(
            None,
            request(dir.path().join("note.txt").display().to_string()),
        )
        .await
        .unwrap();

        assert_eq!(response.content, "hello");
        assert_eq!(response.size, 5);
        assert!(response.etag.starts_with('"'));
    }

    #[tokio::test]
    async fn rejects_content_that_is_not_utf8() {
        let dir = TempDir::new();
        std::fs::write(dir.path().join("blob.bin"), b"\xff").unwrap();

        let error = read(
            None,
            request(dir.path().join("blob.bin").display().to_string()),
        )
        .await
        .unwrap_err();

        assert!(matches!(error, ReadError::NotUtf8));
    }

    #[tokio::test]
    async fn rejects_a_directory_and_a_missing_file() {
        let dir = TempDir::new();

        let error = read(None, request(dir.path().display().to_string()))
            .await
            .unwrap_err();
        assert!(matches!(error, ReadError::NotAFile(_)));

        let error = read(
            None,
            request(dir.path().join("missing").display().to_string()),
        )
        .await
        .unwrap_err();
        assert!(matches!(error, ReadError::NotFound(_)));
    }

    #[tokio::test]
    async fn requires_an_absolute_path_in_direct_mode() {
        let error = read(None, request("relative.txt")).await.unwrap_err();

        assert!(matches!(error, ReadError::InvalidPath { .. }));
    }

    #[tokio::test]
    async fn reads_a_workspace_relative_path() {
        let dir = TempDir::new();
        std::fs::write(dir.path().join("note.txt"), "hi").unwrap();
        let workspace = registered(&dir).await;

        let response = read(Some(&workspace), request("note.txt")).await.unwrap();

        assert_eq!(response.path, "note.txt");
        assert_eq!(response.content, "hi");
    }

    #[tokio::test]
    async fn rejects_a_workspace_path_that_is_absolute() {
        let dir = TempDir::new();
        std::fs::write(dir.path().join("note.txt"), "hi").unwrap();
        let workspace = registered(&dir).await;

        let error = read(
            Some(&workspace),
            request(dir.path().join("note.txt").display().to_string()),
        )
        .await
        .unwrap_err();

        assert!(matches!(error, ReadError::InvalidPath { .. }));
    }

    #[tokio::test]
    async fn rejects_a_workspace_path_that_escapes_through_a_parent() {
        let dir = TempDir::new();
        let outside = TempDir::new();
        std::fs::write(outside.path().join("secret.txt"), "secret").unwrap();
        let workspace = registered(&dir).await;

        let sibling = outside.path().file_name().unwrap().to_string_lossy();
        let error = read(
            Some(&workspace),
            request(format!("../{sibling}/secret.txt")),
        )
        .await
        .unwrap_err();

        assert!(matches!(error, ReadError::InvalidPath { .. }));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rejects_a_workspace_symlink_that_escapes() {
        let dir = TempDir::new();
        let outside = TempDir::new();
        std::fs::write(outside.path().join("secret.txt"), "secret").unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret.txt"), dir.path().join("link"))
            .unwrap();
        let workspace = registered(&dir).await;

        let error = read(Some(&workspace), request("link")).await.unwrap_err();

        assert!(matches!(error, ReadError::InvalidPath { .. }));
    }
}
