//! `fs/readFile` — the implementation shared by the HTTP and MCP adapters.
//!
//! Knows nothing about either transport; `main.rs` maps [`ReadFileError`].

use std::{fmt, io, path::PathBuf};

use tokio::io::{AsyncBufReadExt, BufReader};

use crate::model::{DEFAULT_LIMIT, ReadFileParams, ReadFileResult, TextLine};

/// Why reading a file failed.
#[derive(Debug)]
pub enum ReadFileError {
    /// The requested path was not absolute.
    RelativePath(PathBuf),
    /// `limit` was present but not at least `1`.
    ZeroLimit,
    /// No file exists at the requested path.
    NotFound(PathBuf),
    /// The path exists but is not a regular file.
    NotAFile(PathBuf),
    /// Reading the file failed.
    Io(io::Error),
}

impl fmt::Display for ReadFileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RelativePath(path) => {
                write!(formatter, "path must be absolute: {}", path.display())
            }
            Self::ZeroLimit => write!(formatter, "limit must be at least 1"),
            Self::NotFound(path) => write!(formatter, "file not found: {}", path.display()),
            Self::NotAFile(path) => {
                write!(formatter, "not a regular file: {}", path.display())
            }
            Self::Io(error) => write!(formatter, "failed to read file: {error}"),
        }
    }
}

impl std::error::Error for ReadFileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

/// Read a window of lines from a UTF-8 text file, streaming so that memory use
/// follows the window rather than the file size.
pub async fn read_file(params: &ReadFileParams) -> Result<ReadFileResult, ReadFileError> {
    let path = PathBuf::from(&params.path);
    if !path.is_absolute() {
        return Err(ReadFileError::RelativePath(path));
    }

    let offset = params.offset.unwrap_or(0);
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT);
    if limit == 0 {
        return Err(ReadFileError::ZeroLimit);
    }

    let file = match tokio::fs::File::open(&path).await {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(ReadFileError::NotFound(path));
        }
        Err(error) => return Err(ReadFileError::Io(error)),
    };

    // Metadata from the handle, so a swap between check and read cannot slip
    // through.
    if !file.metadata().await.map_err(ReadFileError::Io)?.is_file() {
        return Err(ReadFileError::NotAFile(path));
    }

    let mut lines = BufReader::new(file).lines();

    // Reaching EOF while skipping means `offset` is past the end: empty window.
    for _ in 0..offset {
        if lines
            .next_line()
            .await
            .map_err(ReadFileError::Io)?
            .is_none()
        {
            return Ok(ReadFileResult {
                path: params.path.clone(),
                lines: Vec::new(),
                truncated: false,
                next_offset: None,
            });
        }
    }

    let mut window = Vec::new();
    for _ in 0..limit {
        match lines.next_line().await.map_err(ReadFileError::Io)? {
            Some(text) => window.push(text),
            None => break,
        }
    }

    // One line past the window distinguishes "ended" from "more to read".
    let truncated = !window.is_empty()
        && lines
            .next_line()
            .await
            .map_err(ReadFileError::Io)?
            .is_some();
    let next_offset = truncated.then_some(offset + window.len());

    let lines = window
        .into_iter()
        .enumerate()
        .map(|(index, text)| TextLine {
            number: offset + index + 1,
            text,
        })
        .collect();

    Ok(ReadFileResult {
        path: params.path.clone(),
        lines,
        truncated,
        next_offset,
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    /// Write `contents` to a uniquely named file under the temp directory.
    fn fixture(name: &str, contents: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "sandbox-toolkit-read-file-{}-{name}.txt",
            std::process::id()
        ));
        std::fs::write(&path, contents).expect("writing fixture failed");
        path
    }

    fn params(path: &Path, offset: Option<usize>, limit: Option<usize>) -> ReadFileParams {
        ReadFileParams {
            path: path.display().to_string(),
            offset,
            limit,
        }
    }

    #[tokio::test]
    async fn reads_a_window_and_reports_more() {
        let path = fixture("window", "a\nb\nc\nd\ne\n");

        let result = read_file(&params(&path, Some(1), Some(2))).await.unwrap();

        assert_eq!(
            result.lines,
            vec![
                TextLine {
                    number: 2,
                    text: "b".into()
                },
                TextLine {
                    number: 3,
                    text: "c".into()
                },
            ]
        );
        assert!(result.truncated);
        assert_eq!(result.next_offset, Some(3));

        std::fs::remove_file(&path).unwrap();
    }

    #[tokio::test]
    async fn final_window_is_not_truncated() {
        let path = fixture("final", "a\nb\nc\nd\ne\n");

        let result = read_file(&params(&path, Some(3), Some(10))).await.unwrap();

        assert_eq!(
            result
                .lines
                .iter()
                .map(|line| &line.text)
                .collect::<Vec<_>>(),
            vec!["d", "e"]
        );
        assert!(!result.truncated);
        assert_eq!(result.next_offset, None);

        std::fs::remove_file(&path).unwrap();
    }

    #[tokio::test]
    async fn offset_is_zero_based_and_numbers_are_one_based() {
        let path = fixture("numbers", "first\nsecond\n");

        let result = read_file(&params(&path, None, None)).await.unwrap();

        assert_eq!(result.lines[0].number, 1);
        assert_eq!(result.lines[0].text, "first");
        assert_eq!(result.lines[1].number, 2);
        assert_eq!(result.lines[1].text, "second");
        assert!(!result.truncated);

        std::fs::remove_file(&path).unwrap();
    }

    #[tokio::test]
    async fn offset_past_eof_yields_an_empty_window() {
        let path = fixture("past-eof", "only\n");

        let result = read_file(&params(&path, Some(5), None)).await.unwrap();

        assert!(result.lines.is_empty());
        assert!(!result.truncated);
        assert_eq!(result.next_offset, None);

        std::fs::remove_file(&path).unwrap();
    }

    #[tokio::test]
    async fn crlf_newlines_are_stripped() {
        let path = fixture("crlf", "a\r\nb\r\n");

        let result = read_file(&params(&path, None, None)).await.unwrap();

        assert_eq!(result.lines[0].text, "a");
        assert_eq!(result.lines[1].text, "b");

        std::fs::remove_file(&path).unwrap();
    }

    #[tokio::test]
    async fn relative_paths_are_rejected() {
        let error = read_file(&ReadFileParams {
            path: "Cargo.toml".into(),
            offset: None,
            limit: None,
        })
        .await
        .unwrap_err();

        assert!(matches!(error, ReadFileError::RelativePath(_)));
    }

    #[tokio::test]
    async fn zero_limit_is_rejected() {
        let path = fixture("zero-limit", "a\n");

        let error = read_file(&params(&path, None, Some(0))).await.unwrap_err();

        assert!(matches!(error, ReadFileError::ZeroLimit));

        std::fs::remove_file(&path).unwrap();
    }

    #[tokio::test]
    async fn missing_file_is_reported_as_not_found() {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "sandbox-toolkit-read-file-{}-missing.txt",
            std::process::id()
        ));

        let error = read_file(&params(&path, None, None)).await.unwrap_err();

        assert!(matches!(error, ReadFileError::NotFound(_)));
    }

    #[tokio::test]
    async fn directories_are_rejected() {
        let error = read_file(&params(&std::env::temp_dir(), None, None))
            .await
            .unwrap_err();

        assert!(matches!(error, ReadFileError::NotAFile(_)));
    }
}
