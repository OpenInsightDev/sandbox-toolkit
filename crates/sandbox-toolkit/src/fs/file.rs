use std::path::{Path, PathBuf};
use std::time::SystemTime;

use mime_guess::Mime;
use thiserror::Error;
use tokio_util::io::ReaderStream;

use super::TargetFile;
use super::meta::etag;

#[derive(Debug, Error)]
pub(crate) enum ReadFileError {
    #[error("file not found: {0}")]
    NotFound(String),
    #[error("target is not a file: {0}")]
    NotAFile(String),
    #[error("failed to read file: {0}")]
    Io(#[from] std::io::Error),
}

pub(crate) enum DownloadMode {
    Direct,
    Stream,
}

const STREAM_BUFFER_SIZE: usize = 64 * 1024;

const FALLBACK_CONTENT_TYPE: Mime = mime_guess::mime::APPLICATION_OCTET_STREAM;

pub(crate) struct PreparedFile {
    pub(crate) content: FileContent,
    pub(crate) headers: FileHeaders,
}

pub(crate) enum FileContent {
    Inline(Vec<u8>),
    Stream(ReaderStream<tokio::fs::File>),
}

/// Gathered before any byte is sent, so a response carries a consistent view of
/// the file even when the content streams.
pub(crate) struct FileHeaders {
    pub(crate) content_type: Mime,
    pub(crate) content_length: u64,
    pub(crate) etag: String,
    pub(crate) last_modified: Option<SystemTime>,
}

pub(crate) async fn prepare_download(
    target: &TargetFile,
    mode: DownloadMode,
) -> Result<PreparedFile, ReadFileError> {
    let path = checked_target_file(target).await?;
    let headers = stat_file(&path).await?;

    let content = match mode {
        DownloadMode::Direct => FileContent::Inline(tokio::fs::read(&path).await?),
        DownloadMode::Stream => {
            // Opening here is what checks read permission before the response starts.
            let file = tokio::fs::File::open(&path).await?;
            FileContent::Stream(ReaderStream::with_capacity(file, STREAM_BUFFER_SIZE))
        }
    };

    Ok(PreparedFile { content, headers })
}

pub(crate) async fn probe_file(target: &TargetFile) -> Result<FileHeaders, ReadFileError> {
    let path = checked_target_file(target).await?;
    stat_file(&path).await
}

async fn stat_file(path: &Path) -> Result<FileHeaders, ReadFileError> {
    let metadata = tokio::fs::metadata(path).await?;

    Ok(FileHeaders {
        content_type: mime_guess::from_path(path)
            .first()
            .unwrap_or(FALLBACK_CONTENT_TYPE),
        content_length: metadata.len(),
        etag: etag(&metadata),
        last_modified: metadata.modified().ok(),
    })
}

async fn checked_target_file(target_file: &TargetFile) -> Result<PathBuf, ReadFileError> {
    let path = target_file.path();
    let metadata = tokio::fs::metadata(&path).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ReadFileError::NotFound(path.display().to_string())
        } else {
            ReadFileError::Io(error)
        }
    })?;
    if metadata.is_dir() {
        return Err(ReadFileError::NotAFile(path.display().to_string()));
    }

    Ok(path)
}

#[cfg(test)]
mod tests {
    use tokio_stream::StreamExt;

    use crate::workspace::registry::test_support::TempDir;

    use super::*;

    #[tokio::test]
    async fn direct_mode_buffers_the_whole_file() {
        let dir = TempDir::new();
        let content: Vec<u8> = (0..10_000u32)
            .flat_map(|value| value.to_be_bytes())
            .collect();
        tokio::fs::write(dir.path().join("big.bin"), &content)
            .await
            .unwrap();

        let prepared = prepare_download(
            &TargetFile::Absolute(dir.path().join("big.bin")),
            DownloadMode::Direct,
        )
        .await
        .unwrap();

        assert_eq!(prepared.headers.content_length, 40_000);
        assert_eq!(
            prepared.headers.content_type,
            mime_guess::mime::APPLICATION_OCTET_STREAM
        );
        assert!(prepared.headers.etag.starts_with('"'));
        assert!(prepared.headers.last_modified.is_some());
        let FileContent::Inline(bytes) = prepared.content else {
            panic!("direct mode must buffer the whole file");
        };
        assert_eq!(bytes, content);
    }

    #[tokio::test]
    async fn stream_mode_sends_the_whole_file() {
        let dir = TempDir::new();
        tokio::fs::write(dir.path().join("a.txt"), "hello")
            .await
            .unwrap();

        let prepared = prepare_download(
            &TargetFile::Absolute(dir.path().join("a.txt")),
            DownloadMode::Stream,
        )
        .await
        .unwrap();

        assert_eq!(prepared.headers.content_length, 5);
        assert_eq!(prepared.headers.content_type, mime_guess::mime::TEXT_PLAIN);
        let FileContent::Stream(stream) = prepared.content else {
            panic!("stream mode must stream the file");
        };
        let mut body = Vec::new();
        tokio::pin!(stream);
        while let Some(chunk) = stream.next().await {
            body.extend_from_slice(&chunk.unwrap());
        }
        assert_eq!(body, b"hello");
    }

    #[tokio::test]
    async fn probe_reports_facts_without_a_body() {
        let dir = TempDir::new();
        tokio::fs::write(dir.path().join("a.txt"), "hello")
            .await
            .unwrap();

        let facts = probe_file(&TargetFile::Absolute(dir.path().join("a.txt")))
            .await
            .unwrap();

        assert_eq!(facts.content_length, 5);
        assert_eq!(facts.content_type, mime_guess::mime::TEXT_PLAIN);
        assert!(facts.last_modified.is_some());
    }

    #[tokio::test]
    async fn follows_a_symlink_to_a_file() {
        let dir = TempDir::new();
        tokio::fs::write(dir.path().join("a.txt"), "hello")
            .await
            .unwrap();
        tokio::fs::symlink(dir.path().join("a.txt"), dir.path().join("link"))
            .await
            .unwrap();

        let prepared = prepare_download(
            &TargetFile::Absolute(dir.path().join("link")),
            DownloadMode::Direct,
        )
        .await
        .unwrap();

        assert_eq!(prepared.headers.content_length, 5);
    }

    #[tokio::test]
    async fn rejects_a_directory_and_a_missing_file() {
        let dir = TempDir::new();

        let result = prepare_download(
            &TargetFile::Absolute(dir.path().to_owned()),
            DownloadMode::Direct,
        )
        .await;
        assert!(matches!(result, Err(ReadFileError::NotAFile(_))));

        let result = prepare_download(
            &TargetFile::Absolute(dir.path().join("missing")),
            DownloadMode::Direct,
        )
        .await;
        assert!(matches!(result, Err(ReadFileError::NotFound(_))));
    }
}
