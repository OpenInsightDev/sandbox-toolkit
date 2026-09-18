//! Filesystem operations (`fs/*`) — the implementations shared by the HTTP and
//! MCP adapters. The operations mirror the core WebDAV methods: `GET` is
//! [`read_file`], `PROPFIND` is [`stat`] and [`list`], `MKCOL` is [`mkdir`],
//! `PUT` is [`write_file`], `DELETE` is [`remove`], and `COPY` and `MOVE` are
//! [`copy`] and [`move_`].

use std::{
    io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::model::{
    CopyParams, CopyResult, DEFAULT_LIMIT, ListParams, ListResult, MkdirParams, MkdirResult,
    MoveParams, MoveResult, ReadFileParams, ReadFileResult, RemoveParams, RemoveResult, Resource,
    ResourceKind, StatParams, StatResult, TextLine, WriteFileParams, WriteFileResult,
};

/// Why a filesystem operation failed.
#[derive(Debug, Error)]
pub enum FsError {
    /// The requested path was empty or only whitespace.
    #[error("path must not be empty")]
    EmptyPath,
    /// The requested path was not absolute.
    #[error("path must be absolute: {}", .0.display())]
    RelativePath(PathBuf),
    /// `limit` was present but not at least `1`.
    #[error("limit must be at least 1")]
    ZeroLimit,
    /// No entry exists at the requested path.
    #[error("no such file or directory: {}", .0.display())]
    NotFound(PathBuf),
    /// An entry already exists at a path that must not be overwritten.
    #[error("already exists: {}", .0.display())]
    AlreadyExists(PathBuf),
    /// The path exists but is not a regular file.
    #[error("not a regular file: {}", .0.display())]
    NotAFile(PathBuf),
    /// The path exists but is not a directory.
    #[error("not a directory: {}", .0.display())]
    NotADirectory(PathBuf),
    /// The path is a directory where a file was expected.
    #[error("is a directory: {}", .0.display())]
    IsADirectory(PathBuf),
    /// A directory that must be removed still has members.
    #[error("directory is not empty: {}", .0.display())]
    DirectoryNotEmpty(PathBuf),
    /// The source and destination name the same path.
    #[error("source and destination must differ: {}", .0.display())]
    SamePath(PathBuf),
    /// The destination lies inside the source, which `copy` cannot represent.
    #[error("destination is inside the source: {}", .0.display())]
    DestinationInsideSource(PathBuf),
    /// Overwriting the destination would delete the source because the
    /// destination is one of its ancestors.
    #[error("overwriting would remove the source: {}", .0.display())]
    SourceInsideDestination(PathBuf),
    /// The filesystem reported an error we could not classify further.
    #[error("filesystem operation failed: {0}")]
    Io(#[from] io::Error),
    /// A blocking filesystem task panicked instead of returning.
    #[error("filesystem task failed")]
    Task(#[from] tokio::task::JoinError),
}

/// Resolve a request path to an absolute [`PathBuf`], rejecting empty and
/// relative paths before they reach the filesystem.
fn absolute(path: &str) -> Result<PathBuf, FsError> {
    if path.trim().is_empty() {
        return Err(FsError::EmptyPath);
    }

    let path = PathBuf::from(path);
    if !path.is_absolute() {
        return Err(FsError::RelativePath(path));
    }

    Ok(path)
}

/// Read metadata for `path` without following a final symbolic link.
async fn link_metadata(path: &Path) -> Result<std::fs::Metadata, FsError> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) => Ok(metadata),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Err(FsError::NotFound(path.to_path_buf()))
        }
        Err(error) => Err(FsError::Io(error)),
    }
}

/// Describe `path` using metadata that was already read for it.
async fn resource_from(path: &Path, metadata: &std::fs::Metadata) -> Result<Resource, FsError> {
    let file_type = metadata.file_type();
    let kind = if file_type.is_symlink() {
        ResourceKind::Symlink
    } else if file_type.is_dir() {
        ResourceKind::Directory
    } else {
        ResourceKind::File
    };

    let target = if kind == ResourceKind::Symlink {
        tokio::fs::read_link(path)
            .await
            .ok()
            .map(|target| target.to_string_lossy().into_owned())
    } else {
        None
    };

    Ok(Resource {
        path: path.display().to_string(),
        name: name_of(path),
        kind,
        size: metadata.len(),
        modified_at: metadata.modified().ok().and_then(millis),
        created_at: metadata.created().ok().and_then(millis),
        read_only: metadata.permissions().readonly(),
        target,
    })
}

/// Describe `path`, following no symbolic link.
async fn resource(path: &Path) -> Result<Resource, FsError> {
    let metadata = link_metadata(path).await?;
    resource_from(path, &metadata).await
}

/// The last path component, or an empty string for a path without one.
fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Convert a timestamp to milliseconds since the Unix epoch, dropping values
/// the platform cannot express.
fn millis(time: SystemTime) -> Option<i64> {
    let elapsed = time.duration_since(UNIX_EPOCH).ok()?;
    i64::try_from(elapsed.as_millis()).ok()
}

/// Whether a directory has no members.
async fn dir_is_empty(path: &Path) -> Result<bool, FsError> {
    let mut reader = tokio::fs::read_dir(path).await?;
    Ok(reader.next_entry().await?.is_none())
}

/// Remove whatever entry already sits at `path`, directory or not.
async fn remove_entry(path: &Path, metadata: &std::fs::Metadata) -> Result<(), FsError> {
    if metadata.is_dir() {
        tokio::fs::remove_dir_all(path).await?;
    } else {
        tokio::fs::remove_file(path).await?;
    }
    Ok(())
}

/// Reject a source and destination that name the same path.
fn ensure_distinct(source: &Path, destination: &Path) -> Result<(), FsError> {
    if source == destination {
        return Err(FsError::SamePath(source.to_path_buf()));
    }
    Ok(())
}

/// Read a window of lines from a UTF-8 text file, streaming so that memory use
/// follows the window rather than the file size.
pub async fn read_file(params: &ReadFileParams) -> Result<ReadFileResult, FsError> {
    let path = absolute(&params.path)?;

    let offset = params.offset.unwrap_or(0);
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT);
    if limit == 0 {
        return Err(FsError::ZeroLimit);
    }

    let file = match tokio::fs::File::open(&path).await {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(FsError::NotFound(path));
        }
        Err(error) => return Err(FsError::Io(error)),
    };

    if !file.metadata().await?.is_file() {
        return Err(FsError::NotAFile(path));
    }

    let mut lines = BufReader::new(file).lines();

    for _ in 0..offset {
        if lines.next_line().await?.is_none() {
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
        match lines.next_line().await? {
            Some(text) => window.push(text),
            None => break,
        }
    }

    let truncated = !window.is_empty() && lines.next_line().await?.is_some();
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

/// Describe one entry, the analog of a `PROPFIND` with `Depth: 0`.
pub async fn stat(params: &StatParams) -> Result<StatResult, FsError> {
    let path = absolute(&params.path)?;
    let resource = resource(&path).await?;
    Ok(StatResult { resource })
}

/// List a directory's immediate members, the analog of a `PROPFIND` with
/// `Depth: 1`.
pub async fn list(params: &ListParams) -> Result<ListResult, FsError> {
    let path = absolute(&params.path)?;

    let metadata = match tokio::fs::metadata(&path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(FsError::NotFound(path));
        }
        Err(error) => return Err(FsError::Io(error)),
    };
    if !metadata.is_dir() {
        return Err(FsError::NotADirectory(path));
    }

    let mut reader = tokio::fs::read_dir(&path).await?;
    let mut entries = Vec::new();
    while let Some(entry) = reader.next_entry().await? {
        let entry_path = entry.path();
        let metadata = match link_metadata(&entry_path).await {
            Ok(metadata) => metadata,
            Err(FsError::NotFound(_)) => continue,
            Err(error) => return Err(error),
        };
        entries.push(resource_from(&entry_path, &metadata).await?);
    }
    entries.sort_by(|left, right| left.name.cmp(&right.name));

    Ok(ListResult {
        path: params.path.clone(),
        entries,
    })
}

/// Create a directory, the analog of `MKCOL`.
pub async fn mkdir(params: &MkdirParams) -> Result<MkdirResult, FsError> {
    let path = absolute(&params.path)?;
    let recursive = params.recursive.unwrap_or(false);

    if !recursive && tokio::fs::symlink_metadata(&path).await.is_ok() {
        return Err(FsError::AlreadyExists(path));
    }

    if recursive {
        tokio::fs::create_dir_all(&path).await?;
    } else {
        tokio::fs::create_dir(&path).await?;
    }

    let resource = resource(&path).await?;
    Ok(MkdirResult { resource })
}

/// Create or replace a file, the analog of `PUT`.
pub async fn write_file(params: &WriteFileParams) -> Result<WriteFileResult, FsError> {
    let path = absolute(&params.path)?;

    if let Ok(metadata) = tokio::fs::metadata(&path).await
        && metadata.is_dir()
    {
        return Err(FsError::IsADirectory(path));
    }

    let mut options = tokio::fs::OpenOptions::new();
    options.write(true);
    if params.append.unwrap_or(false) {
        options.append(true).create(true);
    } else {
        options.create(true).truncate(true);
    }

    let mut file = options.open(&path).await?;
    file.write_all(params.contents.as_bytes()).await?;
    file.flush().await?;

    let resource = resource(&path).await?;
    Ok(WriteFileResult { resource })
}

/// Remove a file, symbolic link or directory, the analog of `DELETE`.
pub async fn remove(params: &RemoveParams) -> Result<RemoveResult, FsError> {
    let path = absolute(&params.path)?;
    let metadata = link_metadata(&path).await?;

    if metadata.is_dir() {
        if params.recursive.unwrap_or(false) {
            tokio::fs::remove_dir_all(&path).await?;
        } else {
            if !dir_is_empty(&path).await? {
                return Err(FsError::DirectoryNotEmpty(path));
            }
            tokio::fs::remove_dir(&path).await?;
        }
    } else {
        tokio::fs::remove_file(&path).await?;
    }

    Ok(RemoveResult {
        path: params.path.clone(),
    })
}

/// Copy a file, symbolic link or directory, the analog of `COPY`. Directories
/// are copied recursively and symbolic links are recreated rather than
/// followed, so a link cycle cannot make the copy loop.
pub async fn copy(params: &CopyParams) -> Result<CopyResult, FsError> {
    let source = absolute(&params.source)?;
    let destination = absolute(&params.destination)?;
    ensure_distinct(&source, &destination)?;

    link_metadata(&source).await?;

    if destination.starts_with(&source) {
        return Err(FsError::DestinationInsideSource(destination));
    }

    if let Ok(metadata) = tokio::fs::symlink_metadata(&destination).await {
        if !params.overwrite.unwrap_or(false) {
            return Err(FsError::AlreadyExists(destination));
        }
        if source.starts_with(&destination) {
            return Err(FsError::SourceInsideDestination(destination));
        }
        remove_entry(&destination, &metadata).await?;
    }

    let source_copy = source.clone();
    let destination_copy = destination.clone();
    tokio::task::spawn_blocking(move || copy_entry(&source_copy, &destination_copy)).await??;

    let resource = resource(&destination).await?;
    Ok(CopyResult { resource })
}

/// Move or rename a file, symbolic link or directory, the analog of `MOVE`.
pub async fn move_(params: &MoveParams) -> Result<MoveResult, FsError> {
    let source = absolute(&params.source)?;
    let destination = absolute(&params.destination)?;
    ensure_distinct(&source, &destination)?;

    link_metadata(&source).await?;

    if let Ok(metadata) = tokio::fs::symlink_metadata(&destination).await {
        if !params.overwrite.unwrap_or(false) {
            return Err(FsError::AlreadyExists(destination));
        }
        if source.starts_with(&destination) {
            return Err(FsError::SourceInsideDestination(destination));
        }
        remove_entry(&destination, &metadata).await?;
    }

    tokio::fs::rename(&source, &destination).await?;

    let resource = resource(&destination).await?;
    Ok(MoveResult { resource })
}

/// Recursively copy one entry, recreating symbolic links instead of following
/// them. Runs on a blocking thread so a deep tree cannot stall the runtime.
fn copy_entry(source: &Path, destination: &Path) -> Result<(), FsError> {
    let metadata = std::fs::symlink_metadata(source)?;
    let file_type = metadata.file_type();

    if file_type.is_symlink() {
        let target = std::fs::read_link(source)?;
        std::os::unix::fs::symlink(&target, destination)?;
    } else if file_type.is_dir() {
        std::fs::create_dir(destination)?;
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            copy_entry(&entry.path(), &destination.join(entry.file_name()))?;
        }
    } else {
        std::fs::copy(source, destination)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a unique, empty scratch directory for one test.
    fn scratch(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("sandbox-toolkit-fs-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("creating the scratch directory failed");
        path
    }

    /// Write `contents` to `name` inside `dir` and return that file's path.
    fn fixture(dir: &Path, name: &str, contents: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, contents).expect("writing the fixture failed");
        path
    }

    fn names(resources: &[Resource]) -> Vec<String> {
        resources
            .iter()
            .map(|resource| resource.name.clone())
            .collect()
    }

    #[tokio::test]
    async fn reads_a_window_and_reports_more() {
        let dir = scratch("read-window");
        let path = fixture(&dir, "a.txt", "a\nb\nc\nd\ne\n");

        let result = read_file(&ReadFileParams {
            path: path.display().to_string(),
            offset: Some(1),
            limit: Some(2),
        })
        .await
        .unwrap();

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

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn final_window_is_not_truncated() {
        let dir = scratch("read-final");
        let path = fixture(&dir, "a.txt", "a\nb\nc\nd\ne\n");

        let result = read_file(&ReadFileParams {
            path: path.display().to_string(),
            offset: Some(3),
            limit: Some(10),
        })
        .await
        .unwrap();

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

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn offset_is_zero_based_and_numbers_are_one_based() {
        let dir = scratch("read-numbers");
        let path = fixture(&dir, "a.txt", "first\nsecond\n");

        let result = read_file(&ReadFileParams {
            path: path.display().to_string(),
            offset: None,
            limit: None,
        })
        .await
        .unwrap();

        assert_eq!(result.lines[0].number, 1);
        assert_eq!(result.lines[0].text, "first");
        assert_eq!(result.lines[1].number, 2);
        assert_eq!(result.lines[1].text, "second");
        assert!(!result.truncated);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn offset_past_eof_yields_an_empty_window() {
        let dir = scratch("read-past-eof");
        let path = fixture(&dir, "a.txt", "only\n");

        let result = read_file(&ReadFileParams {
            path: path.display().to_string(),
            offset: Some(5),
            limit: None,
        })
        .await
        .unwrap();

        assert!(result.lines.is_empty());
        assert!(!result.truncated);
        assert_eq!(result.next_offset, None);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn crlf_newlines_are_stripped() {
        let dir = scratch("read-crlf");
        let path = fixture(&dir, "a.txt", "a\r\nb\r\n");

        let result = read_file(&ReadFileParams {
            path: path.display().to_string(),
            offset: None,
            limit: None,
        })
        .await
        .unwrap();

        assert_eq!(result.lines[0].text, "a");
        assert_eq!(result.lines[1].text, "b");

        std::fs::remove_dir_all(&dir).unwrap();
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

        assert!(matches!(error, FsError::RelativePath(_)));
    }

    #[tokio::test]
    async fn empty_paths_are_rejected() {
        let error = read_file(&ReadFileParams {
            path: "  ".into(),
            offset: None,
            limit: None,
        })
        .await
        .unwrap_err();

        assert!(matches!(error, FsError::EmptyPath));
    }

    #[tokio::test]
    async fn zero_limit_is_rejected() {
        let dir = scratch("read-zero-limit");
        let path = fixture(&dir, "a.txt", "a\n");

        let error = read_file(&ReadFileParams {
            path: path.display().to_string(),
            offset: None,
            limit: Some(0),
        })
        .await
        .unwrap_err();

        assert!(matches!(error, FsError::ZeroLimit));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn missing_file_is_reported_as_not_found() {
        let dir = scratch("read-missing");

        let error = read_file(&ReadFileParams {
            path: dir.join("missing.txt").display().to_string(),
            offset: None,
            limit: None,
        })
        .await
        .unwrap_err();

        assert!(matches!(error, FsError::NotFound(_)));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn directories_are_rejected() {
        let dir = scratch("read-directory");

        let error = read_file(&ReadFileParams {
            path: dir.display().to_string(),
            offset: None,
            limit: None,
        })
        .await
        .unwrap_err();

        assert!(matches!(error, FsError::NotAFile(_)));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn stat_describes_a_file() {
        let dir = scratch("stat-file");
        let path = fixture(&dir, "a.txt", "hello");

        let result = stat(&StatParams {
            path: path.display().to_string(),
        })
        .await
        .unwrap();

        assert_eq!(result.resource.kind, ResourceKind::File);
        assert_eq!(result.resource.name, "a.txt");
        assert_eq!(result.resource.size, 5);
        assert_eq!(result.resource.path, path.display().to_string());
        assert!(!result.resource.read_only);
        assert!(result.resource.target.is_none());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn stat_reports_a_symbolic_link_without_following_it() {
        let dir = scratch("stat-symlink");
        let target = fixture(&dir, "target.txt", "hello");
        let link = dir.join("link.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let result = stat(&StatParams {
            path: link.display().to_string(),
        })
        .await
        .unwrap();

        assert_eq!(result.resource.kind, ResourceKind::Symlink);
        let expected = target.display().to_string();
        assert_eq!(result.resource.target.as_deref(), Some(expected.as_str()));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn stat_reports_a_missing_entry_as_not_found() {
        let dir = scratch("stat-missing");

        let error = stat(&StatParams {
            path: dir.join("nope.txt").display().to_string(),
        })
        .await
        .unwrap_err();

        assert!(matches!(error, FsError::NotFound(_)));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn list_returns_sorted_immediate_members() {
        let dir = scratch("list");
        fixture(&dir, "b.txt", "b");
        fixture(&dir, "a.txt", "a");
        std::fs::create_dir(dir.join("c")).unwrap();

        let result = list(&ListParams {
            path: dir.display().to_string(),
        })
        .await
        .unwrap();

        assert_eq!(result.path, dir.display().to_string());
        assert_eq!(names(&result.entries), vec!["a.txt", "b.txt", "c"]);
        assert_eq!(result.entries[2].kind, ResourceKind::Directory);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn list_rejects_a_file() {
        let dir = scratch("list-file");
        let path = fixture(&dir, "a.txt", "a");

        let error = list(&ListParams {
            path: path.display().to_string(),
        })
        .await
        .unwrap_err();

        assert!(matches!(error, FsError::NotADirectory(_)));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn mkdir_requires_existing_parents_unless_recursive() {
        let dir = scratch("mkdir");
        let nested = dir.join("a/b/c");
        let nested_path = nested.display().to_string();

        let error = mkdir(&MkdirParams {
            path: nested_path.clone(),
            recursive: None,
        })
        .await
        .unwrap_err();
        assert!(matches!(error, FsError::Io(_)));

        let result = mkdir(&MkdirParams {
            path: nested_path,
            recursive: Some(true),
        })
        .await
        .unwrap();
        assert_eq!(result.resource.kind, ResourceKind::Directory);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn mkdir_refuses_an_existing_path() {
        let dir = scratch("mkdir-existing");
        std::fs::create_dir(dir.join("a")).unwrap();

        let error = mkdir(&MkdirParams {
            path: dir.join("a").display().to_string(),
            recursive: Some(false),
        })
        .await
        .unwrap_err();

        assert!(matches!(error, FsError::AlreadyExists(_)));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn write_file_creates_replaces_and_appends() {
        let dir = scratch("write");
        let path = fixture(&dir, "a.txt", "");
        let path_string = path.display().to_string();

        write_file(&WriteFileParams {
            path: path_string.clone(),
            contents: "one".into(),
            append: None,
        })
        .await
        .unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one");

        write_file(&WriteFileParams {
            path: path_string.clone(),
            contents: "two".into(),
            append: None,
        })
        .await
        .unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "two");

        write_file(&WriteFileParams {
            path: path_string,
            contents: "three".into(),
            append: Some(true),
        })
        .await
        .unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "twothree");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn write_file_rejects_a_directory() {
        let dir = scratch("write-directory");

        let error = write_file(&WriteFileParams {
            path: dir.display().to_string(),
            contents: "x".into(),
            append: None,
        })
        .await
        .unwrap_err();

        assert!(matches!(error, FsError::IsADirectory(_)));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn remove_deletes_entries_and_guards_non_empty_directories() {
        let dir = scratch("remove");
        let file = fixture(&dir, "a.txt", "a");

        remove(&RemoveParams {
            path: file.display().to_string(),
            recursive: None,
        })
        .await
        .unwrap();
        assert!(!file.exists());

        fixture(&dir, "b.txt", "b");
        let error = remove(&RemoveParams {
            path: dir.display().to_string(),
            recursive: None,
        })
        .await
        .unwrap_err();
        assert!(matches!(error, FsError::DirectoryNotEmpty(_)));

        let result = remove(&RemoveParams {
            path: dir.display().to_string(),
            recursive: Some(true),
        })
        .await
        .unwrap();
        assert_eq!(result.path, dir.display().to_string());
        assert!(!dir.exists());
    }

    #[tokio::test]
    async fn remove_deletes_a_symbolic_link_not_its_target() {
        let dir = scratch("remove-symlink");
        let target = fixture(&dir, "target.txt", "hello");
        let link = dir.join("link.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        remove(&RemoveParams {
            path: link.display().to_string(),
            recursive: None,
        })
        .await
        .unwrap();

        assert!(!link.exists());
        assert!(target.exists());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn copy_duplicates_a_tree_and_recreates_links() {
        let dir = scratch("copy");
        let source = dir.join("src");
        std::fs::create_dir(&source).unwrap();
        fixture(&source, "a.txt", "a");
        std::fs::create_dir(source.join("nested")).unwrap();
        fixture(&source.join("nested"), "b.txt", "b");
        std::os::unix::fs::symlink(source.join("a.txt"), source.join("link")).unwrap();

        let destination = dir.join("dst");
        let result = copy(&CopyParams {
            source: source.display().to_string(),
            destination: destination.display().to_string(),
            overwrite: None,
        })
        .await
        .unwrap();

        assert_eq!(result.resource.kind, ResourceKind::Directory);
        assert_eq!(
            std::fs::read_to_string(destination.join("nested/b.txt")).unwrap(),
            "b"
        );
        assert!(
            std::fs::symlink_metadata(destination.join("link"))
                .unwrap()
                .file_type()
                .is_symlink()
        );

        let error = copy(&CopyParams {
            source: source.display().to_string(),
            destination: destination.display().to_string(),
            overwrite: None,
        })
        .await
        .unwrap_err();
        assert!(matches!(error, FsError::AlreadyExists(_)));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn copy_can_replace_an_existing_destination() {
        let dir = scratch("copy-overwrite");
        let source = fixture(&dir, "a.txt", "new");
        let destination = fixture(&dir, "b.txt", "old");

        let result = copy(&CopyParams {
            source: source.display().to_string(),
            destination: destination.display().to_string(),
            overwrite: Some(true),
        })
        .await
        .unwrap();

        assert_eq!(result.resource.name, "b.txt");
        assert_eq!(std::fs::read_to_string(&destination).unwrap(), "new");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn copy_refuses_a_destination_inside_the_source() {
        let dir = scratch("copy-inside");
        let source = dir.join("src");
        std::fs::create_dir(&source).unwrap();

        let error = copy(&CopyParams {
            source: source.display().to_string(),
            destination: source.join("dst").display().to_string(),
            overwrite: None,
        })
        .await
        .unwrap_err();

        assert!(matches!(error, FsError::DestinationInsideSource(_)));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn move_relocates_and_can_overwrite() {
        let dir = scratch("move");
        let source = fixture(&dir, "a.txt", "a");
        let destination = dir.join("b.txt");

        let result = move_(&MoveParams {
            source: source.display().to_string(),
            destination: destination.display().to_string(),
            overwrite: None,
        })
        .await
        .unwrap();

        assert_eq!(result.resource.name, "b.txt");
        assert!(!source.exists());
        assert_eq!(std::fs::read_to_string(&destination).unwrap(), "a");

        fixture(&dir, "a.txt", "again");
        let error = move_(&MoveParams {
            source: source.display().to_string(),
            destination: destination.display().to_string(),
            overwrite: Some(false),
        })
        .await
        .unwrap_err();
        assert!(matches!(error, FsError::AlreadyExists(_)));

        move_(&MoveParams {
            source: source.display().to_string(),
            destination: destination.display().to_string(),
            overwrite: Some(true),
        })
        .await
        .unwrap();
        assert_eq!(std::fs::read_to_string(&destination).unwrap(), "again");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn move_rejects_identical_paths() {
        let dir = scratch("move-same");
        let path = fixture(&dir, "a.txt", "a");

        let error = move_(&MoveParams {
            source: path.display().to_string(),
            destination: path.display().to_string(),
            overwrite: None,
        })
        .await
        .unwrap_err();

        assert!(matches!(error, FsError::SamePath(_)));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn move_missing_source_is_reported_as_not_found() {
        let dir = scratch("move-missing");

        let error = move_(&MoveParams {
            source: dir.join("nope.txt").display().to_string(),
            destination: dir.join("dst.txt").display().to_string(),
            overwrite: None,
        })
        .await
        .unwrap_err();

        assert!(matches!(error, FsError::NotFound(_)));

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
