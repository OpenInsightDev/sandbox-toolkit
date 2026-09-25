//! The content is carried as a JSON string, so bytes that are not valid UTF-8
//! are rejected rather than encoded.

use thiserror::Error;

use super::meta::etag;
use super::model::{ContentRequest, ContentResponse};
use super::path::{self, PathError};
use crate::workspace::registry::Workspace;

#[derive(Debug, Error)]
pub(crate) enum ReadError {
    #[error("target is not a file: {0}")]
    NotAFile(String),
    #[error("file content is not valid UTF-8; use `QUERY ?type=stream` for binary data")]
    NotUtf8,
    #[error(transparent)]
    Path(#[from] PathError),
    #[error("failed to read the file: {0}")]
    Io(#[from] std::io::Error),
}

pub(crate) async fn read(
    workspace: Option<&Workspace>,
    request: ContentRequest,
) -> Result<ContentResponse, ReadError> {
    let resolved = path::resolve(workspace, &request.path).await?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::model::WorkspaceProperties;
    use crate::workspace::registry::WorkspaceRegistry;
    use crate::workspace::registry::test_support::TempDir;

    fn request(path: impl Into<String>) -> ContentRequest {
        ContentRequest { path: path.into() }
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
    async fn rejects_a_directory() {
        let dir = TempDir::new();

        let error = read(None, request(dir.path().display().to_string()))
            .await
            .unwrap_err();

        assert!(matches!(error, ReadError::NotAFile(_)));
    }

    #[tokio::test]
    async fn reads_a_workspace_relative_path() {
        let dir = TempDir::new();
        std::fs::write(dir.path().join("note.txt"), "hi").unwrap();
        let registry = WorkspaceRegistry::default();
        registry
            .register("docs", &dir.root(), WorkspaceProperties::default())
            .await
            .unwrap();
        let workspace = registry.workspace("docs").unwrap();

        let response = read(Some(&workspace), request("note.txt")).await.unwrap();

        assert_eq!(response.path, "note.txt");
        assert_eq!(response.content, "hi");
    }
}
