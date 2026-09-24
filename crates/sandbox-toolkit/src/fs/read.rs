use std::path::PathBuf;

use ropey::Rope;
use thiserror::Error;

use crate::workspace::registry::TargetFile;

use super::model::ReadFileRequest;

#[derive(Debug, Error)]
pub(crate) enum ReadFileError {
    #[error("file not found: {0}")]
    NotFound(String),
    #[error("invalid file: {0}")]
    InvalidFile(String),
    #[error("failed to read file: {0}")]
    Io(#[from] std::io::Error),
}

pub(crate) async fn read_file(
    target: TargetFile,
    request: ReadFileRequest,
) -> Result<String, ReadFileError> {
    let path = checked_target_file(&target).await?;
    let content = tokio::fs::read_to_string(&path).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ReadFileError::NotFound(path.display().to_string())
        } else {
            ReadFileError::Io(error)
        }
    })?;
    let rope = Rope::from_str(&content);
    let start = request.offset.min(rope.len_lines());
    let end = request
        .limit
        .map(|limit| start.saturating_add(limit).min(rope.len_lines()))
        .unwrap_or(rope.len_lines());

    Ok(rope
        .slice(rope.line_to_char(start)..rope.line_to_char(end))
        .to_string())
}

async fn checked_target_file(target_file: &TargetFile) -> Result<PathBuf, ReadFileError> {
    let path = target_file.path();
    let canonical = tokio::fs::canonicalize(&path).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ReadFileError::NotFound(path.display().to_string())
        } else {
            ReadFileError::Io(error)
        }
    })?;

    if canonical.is_dir() {
        return Err(ReadFileError::InvalidFile(
            "read target is a directory".to_owned(),
        ));
    }
    if let Some(root) = target_file.workspace_root() {
        let root = tokio::fs::canonicalize(root).await?;
        if !canonical.starts_with(root) {
            return Err(ReadFileError::InvalidFile(
                "path escapes workspace".to_owned(),
            ));
        }
    }

    Ok(canonical)
}
