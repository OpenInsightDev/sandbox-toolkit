use std::path::PathBuf;
use std::time::SystemTime;

use mime_guess::Mime;
use thiserror::Error;
use tokio_util::io::ReaderStream;

use super::dir::etag;
use crate::workspace::registry::TargetFile;

#[derive(Debug, Error)]
pub(crate) enum ReadFileError {
    #[error("file not found: {0}")]
    NotFound(String),
    #[error("target is not a file: {0}")]
    NotAFile(String),
    #[error("invalid file: {0}")]
    InvalidFile(String),
    #[error("failed to read file: {0}")]
    Io(#[from] std::io::Error),
}

/// Largest file served as one buffered response body; larger files stream.
pub(crate) const INLINE_BODY_LIMIT: u64 = 8 * 1024 * 1024;

/// Bytes pulled from the file per read while streaming.
const STREAM_BUFFER_SIZE: usize = 64 * 1024;

/// Fallback media type when the extension does not identify one.
const FALLBACK_CONTENT_TYPE: Mime = mime_guess::mime::APPLICATION_OCTET_STREAM;

/// A validated file read, ready to become a response.
pub(crate) struct PreparedFile {
    pub(crate) content: FileContent,
    pub(crate) headers: FileHeaders,
}

pub(crate) enum FileContent {
    Inline(Vec<u8>),
    Stream(ReaderStream<tokio::fs::File>),
}

/// Everything the data-plane headers of a file response carry.
///
/// Gathered before any byte is sent, so a response carries a consistent view
/// of the file even when the content streams.
pub(crate) struct FileHeaders {
    pub(crate) content_type: Mime,
    pub(crate) content_length: u64,
    pub(crate) etag: String,
    pub(crate) last_modified: Option<SystemTime>,
}

impl FileHeaders {
    /// Buffer the file whole, or open it for streaming, by `inline_limit`.
    async fn into_content(
        self,
        path: PathBuf,
        inline_limit: u64,
    ) -> Result<PreparedFile, ReadFileError> {
        let content = if self.content_length <= inline_limit {
            FileContent::Inline(tokio::fs::read(&path).await?)
        } else {
            // Opening here is what checks read permission before the response starts.
            let file = tokio::fs::File::open(&path).await?;
            FileContent::Stream(ReaderStream::with_capacity(file, STREAM_BUFFER_SIZE))
        };

        Ok(PreparedFile {
            content,
            headers: self,
        })
    }
}

pub(crate) async fn prepare_download(
    target: &TargetFile,
    inline_limit: u64,
) -> Result<PreparedFile, ReadFileError> {
    let path = checked_target_file(target).await?;
    stat_file(&path)
        .await?
        .into_content(path, inline_limit)
        .await
}

/// Probe the target without touching its content, as `HEAD` does.
pub(crate) async fn probe_file(target: &TargetFile) -> Result<FileHeaders, ReadFileError> {
    let path = checked_target_file(target).await?;
    stat_file(&path).await
}

async fn stat_file(path: &std::path::Path) -> Result<FileHeaders, ReadFileError> {
    let metadata = tokio::fs::metadata(path).await?;
    let modified = metadata.modified().ok();

    Ok(FileHeaders {
        content_type: detect_content_type(path),
        content_length: metadata.len(),
        etag: etag(&metadata),
        last_modified: modified,
    })
}

/// Detection is by extension only and never sniffs content.
fn detect_content_type(path: &std::path::Path) -> Mime {
    mime_guess::from_path(path)
        .first()
        .unwrap_or(FALLBACK_CONTENT_TYPE)
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
        return Err(ReadFileError::NotAFile(path.display().to_string()));
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

#[cfg(test)]
mod tests {
    use tokio_stream::StreamExt;

    use crate::workspace::registry::test_support::TempDir;

    use super::*;

    #[tokio::test]
    async fn reads_a_small_file_whole() {
        let dir = TempDir::new();
        tokio::fs::write(dir.path().join("a.txt"), "hello")
            .await
            .unwrap();

        let prepared = prepare_download(&TargetFile::Absolute(dir.path().join("a.txt")), 1024)
            .await
            .unwrap();

        assert_eq!(prepared.headers.content_length, 5);
        assert_eq!(prepared.headers.content_type, mime_guess::mime::TEXT_PLAIN);
        assert!(prepared.headers.etag.starts_with('"'));
        assert!(prepared.headers.last_modified.is_some());
        let FileContent::Inline(bytes) = prepared.content else {
            panic!("a file under the inline limit must be buffered");
        };
        assert_eq!(bytes, b"hello");
    }

    #[tokio::test]
    async fn streams_a_file_over_the_inline_limit() {
        let dir = TempDir::new();
        let content: Vec<u8> = (0..10_000u32)
            .flat_map(|value| value.to_be_bytes())
            .collect();
        tokio::fs::write(dir.path().join("big.bin"), &content)
            .await
            .unwrap();

        let prepared = prepare_download(&TargetFile::Absolute(dir.path().join("big.bin")), 1024)
            .await
            .unwrap();

        assert_eq!(prepared.headers.content_length, 40_000);
        assert_eq!(
            prepared.headers.content_type,
            mime_guess::mime::APPLICATION_OCTET_STREAM
        );
        let FileContent::Stream(stream) = prepared.content else {
            panic!("a file over the inline limit must stream");
        };
        let mut body = Vec::new();
        tokio::pin!(stream);
        while let Some(chunk) = stream.next().await {
            body.extend_from_slice(&chunk.unwrap());
        }
        assert_eq!(body, content);
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

        let prepared = prepare_download(&TargetFile::Absolute(dir.path().join("link")), 1024)
            .await
            .unwrap();

        assert_eq!(prepared.headers.content_length, 5);
    }

    #[tokio::test]
    async fn rejects_a_directory_and_a_missing_file() {
        let dir = TempDir::new();

        let result = prepare_download(&TargetFile::Absolute(dir.path().to_owned()), 1024).await;
        assert!(matches!(result, Err(ReadFileError::NotAFile(_))));

        let result =
            prepare_download(&TargetFile::Absolute(dir.path().join("missing")), 1024).await;
        assert!(matches!(result, Err(ReadFileError::NotFound(_))));
    }
}
