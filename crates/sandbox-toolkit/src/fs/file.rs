//! Single-resource operations: the content reads, the metadata and access
//! probes, and the basic file mutations. Conditional requests and the
//! multi-resource copy/move are handled elsewhere.

use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use thiserror::Error;

use super::meta::etag;
use super::model::{
    AccessRequest, AccessResponse, ContentRequest, ContentResponse, CreateSymlinkRequest,
    DeleteRequest, LinesRequest, LinesResponse, MetadataRequest, PatchMetadataRequest,
    RealpathRequest, RealpathResponse, ResourceKind, ResourceMetadata, StreamRequest,
    TruncateRequest, WriteFileRequest,
};
use crate::path::{self, PathError};
use crate::workspace::registry::Workspace;

/// Absent `limit` on a line read uses this page size.
const DEFAULT_LINES: u64 = 1000;

#[derive(Debug, Error)]
pub(crate) enum FileError {
    #[error("target is not a file: {0}")]
    NotAFile(String),
    #[error("file content is not valid UTF-8; use `QUERY ?type=stream` for binary data")]
    NotUtf8,
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("resource already exists: {0}")]
    AlreadyExists(String),
    #[error(transparent)]
    Path(#[from] PathError),
    #[error("failed to access the file: {0}")]
    Io(#[from] std::io::Error),
}

pub(crate) async fn read(
    workspace: Option<&Workspace>,
    request: ContentRequest,
) -> Result<ContentResponse, FileError> {
    let (content, metadata) = read_text(workspace, &request.path).await?;
    let size = content.len() as u64;

    Ok(ContentResponse {
        path: request.path,
        content,
        size,
        etag: etag(&metadata),
    })
}

/// Resolves `path`, requires a file, and returns it with its metadata.
async fn resolve_file(
    workspace: Option<&Workspace>,
    path: &str,
) -> Result<(PathBuf, std::fs::Metadata), FileError> {
    let resolved = path::resolve(workspace, path).await?;
    let metadata = tokio::fs::metadata(&resolved).await?;
    if !metadata.is_file() {
        return Err(FileError::NotAFile(path.to_owned()));
    }

    Ok((resolved, metadata))
}

/// Reads a text file and returns its decoded contents with the metadata observed
/// before the read.
async fn read_text(
    workspace: Option<&Workspace>,
    path: &str,
) -> Result<(String, std::fs::Metadata), FileError> {
    let (resolved, metadata) = resolve_file(workspace, path).await?;
    let bytes = tokio::fs::read(&resolved).await?;
    let content = String::from_utf8(bytes).map_err(|_| FileError::NotUtf8)?;

    Ok((content, metadata))
}

/// An open file ready to be streamed as the raw response body.
pub(crate) struct StreamedFile {
    pub(crate) file: tokio::fs::File,
    pub(crate) size: u64,
    pub(crate) etag: String,
    pub(crate) modified: SystemTime,
    pub(crate) content_type: String,
}

pub(crate) async fn stream(
    workspace: Option<&Workspace>,
    request: StreamRequest,
) -> Result<StreamedFile, FileError> {
    let (resolved, metadata) = resolve_file(workspace, &request.path).await?;

    let size = metadata.len();
    let etag = etag(&metadata);
    let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    let content_type = mime_guess::from_path(&resolved)
        .first_or_octet_stream()
        .to_string();
    let file = tokio::fs::File::open(&resolved).await?;

    Ok(StreamedFile {
        file,
        size,
        etag,
        modified,
        content_type,
    })
}

pub(crate) async fn metadata(
    workspace: Option<&Workspace>,
    request: MetadataRequest,
) -> Result<ResourceMetadata, FileError> {
    let entry = path::resolve_entry(workspace, &request.path).await?;
    let metadata = tokio::fs::symlink_metadata(&entry).await?;

    describe(&entry, &request.path, &metadata).await
}

pub(crate) async fn realpath(
    workspace: Option<&Workspace>,
    request: RealpathRequest,
) -> Result<RealpathResponse, FileError> {
    let resolved = path::resolve(workspace, &request.path).await?;

    Ok(RealpathResponse {
        path: resolved.display().to_string(),
    })
}

pub(crate) async fn access(
    workspace: Option<&Workspace>,
    request: AccessRequest,
) -> Result<AccessResponse, FileError> {
    let resolved = path::resolve(workspace, &request.path).await?;

    let mut access = process_access(&resolved);
    if let Some(workspace) = workspace
        && workspace.require_writable().is_err()
    {
        access.writable = false;
    }

    Ok(access)
}

pub(crate) async fn lines(
    workspace: Option<&Workspace>,
    request: LinesRequest,
) -> Result<LinesResponse, FileError> {
    let limit = request.limit.unwrap_or(DEFAULT_LINES);
    if limit == 0 {
        return Err(FileError::BadRequest("`limit` must be positive".to_owned()));
    }

    let (content, _) = read_text(workspace, &request.path).await?;

    let source: Vec<&str> = content.lines().collect();
    let total = source.len() as u64;
    let start = request.offset.unwrap_or(0).min(total);
    let end = start.saturating_add(limit).min(total);
    let lines = source[start as usize..end as usize]
        .iter()
        .map(|line| (*line).to_owned())
        .collect();

    Ok(LinesResponse {
        lines,
        offset: start,
        truncated: end < total,
    })
}

/// Whether a write created the resource or replaced it.
pub(crate) struct WriteOutcome {
    pub(crate) created: bool,
    pub(crate) metadata: ResourceMetadata,
}

pub(crate) async fn write_file(
    workspace: Option<&Workspace>,
    request: WriteFileRequest,
) -> Result<WriteOutcome, FileError> {
    let entry = path::resolve_entry(workspace, &request.path).await?;

    let created = match tokio::fs::symlink_metadata(&entry).await {
        Ok(existing) => {
            if existing.is_dir() {
                return Err(FileError::NotAFile(request.path.clone()));
            }
            false
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(error) => return Err(error.into()),
    };

    tokio::fs::write(&entry, request.content.as_bytes()).await?;
    let metadata = tokio::fs::symlink_metadata(&entry).await?;

    Ok(WriteOutcome {
        created,
        metadata: describe(&entry, &request.path, &metadata).await?,
    })
}

pub(crate) async fn create_symlink(
    workspace: Option<&Workspace>,
    request: CreateSymlinkRequest,
) -> Result<ResourceMetadata, FileError> {
    let entry = path::resolve_entry(workspace, &request.path).await?;
    if tokio::fs::symlink_metadata(&entry).await.is_ok() {
        return Err(FileError::AlreadyExists(request.path));
    }

    if let Some(workspace) = workspace {
        let parent = entry.parent().unwrap_or(workspace.root());
        let target = Path::new(&request.target);
        let target = if target.is_absolute() {
            target.to_path_buf()
        } else {
            parent.join(target)
        };

        if !path::normalize(&target).starts_with(workspace.root()) {
            return Err(FileError::BadRequest(format!(
                "symlink target escapes the workspace: {}",
                request.target
            )));
        }
    }

    create_link(Path::new(&request.target), &entry)?;
    let metadata = tokio::fs::symlink_metadata(&entry).await?;

    describe(&entry, &request.path, &metadata).await
}

pub(crate) async fn patch_metadata(
    workspace: Option<&Workspace>,
    request: PatchMetadataRequest,
) -> Result<ResourceMetadata, FileError> {
    let entry = path::resolve_entry(workspace, &request.path).await?;
    // Resolve the target up front so a missing resource is reported as not found
    // even when the request changes nothing.
    tokio::fs::symlink_metadata(&entry).await?;

    if let Some(mode) = &request.mode {
        let bits = u32::from_str_radix(mode.strip_prefix("0o").unwrap_or(mode), 8)
            .map_err(|_| FileError::InvalidRequest(format!("invalid mode: {mode}")))?;
        set_permissions(&entry, bits).await?;
    }

    if let Some(modified) = &request.modified_at {
        let time: SystemTime = chrono::DateTime::parse_from_rfc3339(modified)
            .map_err(|_| FileError::InvalidRequest(format!("invalid modified_at: {modified}")))?
            .into();
        set_modified(&entry, time).await?;
    }

    let metadata = tokio::fs::symlink_metadata(&entry).await?;

    describe(&entry, &request.path, &metadata).await
}

pub(crate) async fn truncate(
    workspace: Option<&Workspace>,
    request: TruncateRequest,
) -> Result<ResourceMetadata, FileError> {
    let entry = path::resolve_entry(workspace, &request.path).await?;
    let metadata = tokio::fs::metadata(&entry).await?;
    if !metadata.is_file() {
        return Err(FileError::NotAFile(request.path));
    }

    let file = tokio::fs::OpenOptions::new()
        .write(true)
        .open(&entry)
        .await?;
    file.set_len(request.length).await?;
    drop(file);

    let metadata = tokio::fs::symlink_metadata(&entry).await?;

    describe(&entry, &request.path, &metadata).await
}

pub(crate) async fn delete(
    workspace: Option<&Workspace>,
    request: DeleteRequest,
) -> Result<(), FileError> {
    let force = request.force.unwrap_or(false);

    let entry = match path::resolve_entry(workspace, &request.path).await {
        Ok(entry) => entry,
        Err(PathError::NotFound(_)) if force => return Ok(()),
        Err(error) => return Err(error.into()),
    };

    let metadata = match tokio::fs::symlink_metadata(&entry).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && force => return Ok(()),
        Err(error) => return Err(error.into()),
    };

    if metadata.is_dir() {
        if !request.recursive.unwrap_or(false) {
            return Err(FileError::NotAFile(request.path));
        }
        tokio::fs::remove_dir_all(&entry).await?;
    } else {
        // A symlink is removed itself, never its target.
        tokio::fs::remove_file(&entry).await?;
    }

    Ok(())
}

async fn describe(
    entry: &Path,
    address: &str,
    metadata: &std::fs::Metadata,
) -> Result<ResourceMetadata, FileError> {
    let kind = if metadata.is_symlink() {
        ResourceKind::Symlink
    } else if metadata.is_dir() {
        ResourceKind::Directory
    } else {
        ResourceKind::File
    };

    let mut described = ResourceMetadata {
        name: Path::new(address)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        path: address.to_owned(),
        kind,
        size: if metadata.is_file() {
            metadata.len()
        } else {
            0
        },
        etag: etag(metadata),
        mode: None,
        uid: None,
        gid: None,
        inode: None,
        links: None,
        device: None,
        device_type: None,
        block_size: None,
        blocks: None,
        modified_at: timestamp(metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH)),
        accessed_at: metadata.accessed().ok().map(timestamp),
        // Not every filesystem records a creation time.
        birthtime: metadata.created().ok().map(timestamp),
        target: if kind == ResourceKind::Symlink {
            tokio::fs::read_link(entry)
                .await
                .ok()
                .map(|target| target.to_string_lossy().into_owned())
        } else {
            None
        },
    };

    described.mode = Some(format!("{:04o}", metadata.permissions().mode() & 0o7777));
    described.uid = Some(u64::from(metadata.uid()));
    described.gid = Some(u64::from(metadata.gid()));
    described.inode = Some(metadata.ino());
    described.links = Some(metadata.nlink());
    described.device = Some(metadata.dev());
    described.device_type = Some(metadata.rdev());
    described.block_size = Some(metadata.blksize());
    described.blocks = Some(metadata.blocks());

    Ok(described)
}

fn timestamp(time: SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(time).to_rfc3339()
}

fn process_access(path: &Path) -> AccessResponse {
    // `path` is canonical: it holds no NUL and no symlink left to follow.
    let path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .expect("a canonical path contains no NUL");
    let allowed = |mode| unsafe {
        libc::faccessat(libc::AT_FDCWD, path.as_ptr(), mode, libc::AT_EACCESS) == 0
    };

    AccessResponse {
        readable: allowed(libc::R_OK),
        writable: allowed(libc::W_OK),
        executable: allowed(libc::X_OK),
    }
}

fn create_link(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

async fn set_permissions(path: &Path, bits: u32) -> Result<(), FileError> {
    tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(bits))
        .await
        .map_err(FileError::from)
}

async fn set_modified(path: &Path, time: SystemTime) -> Result<(), FileError> {
    let path = path.to_path_buf();

    tokio::task::spawn_blocking(move || {
        let file = std::fs::OpenOptions::new().write(true).open(&path)?;
        file.set_modified(time)
    })
    .await
    .map_err(|error| FileError::Io(std::io::Error::other(error)))?
    .map_err(FileError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::model::WorkspaceProperties;
    use crate::workspace::registry::WorkspaceRegistry;
    use crate::workspace::registry::test_support::TempDir;

    fn content_request(path: impl Into<String>) -> ContentRequest {
        ContentRequest { path: path.into() }
    }

    #[tokio::test]
    async fn reads_utf8_content_in_direct_mode() {
        let dir = TempDir::new();
        std::fs::write(dir.path().join("note.txt"), "hello").unwrap();

        let response = read(
            None,
            content_request(dir.path().join("note.txt").display().to_string()),
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
            content_request(dir.path().join("blob.bin").display().to_string()),
        )
        .await
        .unwrap_err();

        assert!(matches!(error, FileError::NotUtf8));
    }

    #[tokio::test]
    async fn rejects_a_directory() {
        let dir = TempDir::new();

        let error = read(None, content_request(dir.path().display().to_string()))
            .await
            .unwrap_err();

        assert!(matches!(error, FileError::NotAFile(_)));
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

        let response = read(Some(&workspace), content_request("note.txt"))
            .await
            .unwrap();

        assert_eq!(response.path, "note.txt");
        assert_eq!(response.content, "hi");
    }

    #[tokio::test]
    async fn describes_a_file_and_a_directory() {
        let dir = TempDir::new();
        std::fs::write(dir.path().join("note.txt"), "hello").unwrap();

        let file = metadata(
            None,
            MetadataRequest {
                path: dir.path().join("note.txt").display().to_string(),
            },
        )
        .await
        .unwrap();

        assert_eq!(file.name, "note.txt");
        assert_eq!(file.kind, ResourceKind::File);
        assert_eq!(file.size, 5);
        assert!(file.etag.starts_with('"'));

        let directory = metadata(
            None,
            MetadataRequest {
                path: dir.path().display().to_string(),
            },
        )
        .await
        .unwrap();

        assert_eq!(directory.kind, ResourceKind::Directory);
        assert_eq!(directory.size, 0);
    }

    #[tokio::test]
    async fn describes_a_symlink_with_its_verbatim_target() {
        let dir = TempDir::new();
        std::fs::write(dir.path().join("note.txt"), "hello").unwrap();
        std::os::unix::fs::symlink("note.txt", dir.path().join("link")).unwrap();

        let described = metadata(
            None,
            MetadataRequest {
                path: dir.path().join("link").display().to_string(),
            },
        )
        .await
        .unwrap();

        assert_eq!(described.kind, ResourceKind::Symlink);
        assert_eq!(described.target.as_deref(), Some("note.txt"));
    }

    #[tokio::test]
    async fn realpath_normalizes_direct_paths() {
        let dir = TempDir::new();
        std::fs::write(dir.path().join("note.txt"), "hi").unwrap();

        let response = realpath(
            None,
            RealpathRequest {
                path: dir.path().join("./note.txt").display().to_string(),
            },
        )
        .await
        .unwrap();

        let expected = std::fs::canonicalize(dir.path().join("note.txt")).unwrap();
        assert_eq!(response.path, expected.display().to_string());
    }

    #[tokio::test]
    async fn realpath_returns_an_absolute_path_in_workspace_mode() {
        let dir = TempDir::new();
        std::fs::write(dir.path().join("note.txt"), "hi").unwrap();
        let registry = WorkspaceRegistry::default();
        registry
            .register("docs", &dir.root(), WorkspaceProperties::default())
            .await
            .unwrap();
        let workspace = registry.workspace("docs").unwrap();

        let response = realpath(
            Some(&workspace),
            RealpathRequest {
                path: "note.txt".to_owned(),
            },
        )
        .await
        .unwrap();

        let expected = std::fs::canonicalize(dir.path().join("note.txt")).unwrap();
        assert_eq!(response.path, expected.display().to_string());
        assert!(Path::new(&response.path).is_absolute());
    }

    #[tokio::test]
    async fn access_reports_the_process_permissions() {
        use std::os::unix::fs::PermissionsExt;

        // Root bypasses the permission bits, so the probe cannot be exercised.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }

        let dir = TempDir::new();
        let path = dir.path().join("note.txt");
        std::fs::write(&path, "hi").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();

        let response = access(
            None,
            AccessRequest {
                path: path.display().to_string(),
            },
        )
        .await
        .unwrap();

        assert!(response.readable);
        assert!(!response.writable);
        assert!(!response.executable);
    }

    #[tokio::test]
    async fn lines_window_the_content() {
        let dir = TempDir::new();
        let path = dir.path().join("note.txt");
        std::fs::write(&path, "a\nb\nc\nd").unwrap();

        let response = lines(
            None,
            LinesRequest {
                path: path.display().to_string(),
                offset: Some(1),
                limit: Some(2),
            },
        )
        .await
        .unwrap();

        assert_eq!(response.lines, ["b", "c"]);
        assert_eq!(response.offset, 1);
        assert!(response.truncated);

        let beyond = lines(
            None,
            LinesRequest {
                path: path.display().to_string(),
                offset: Some(10),
                limit: None,
            },
        )
        .await
        .unwrap();

        assert!(beyond.lines.is_empty());
        assert!(!beyond.truncated);
    }

    #[tokio::test]
    async fn writes_then_replaces_a_file() {
        let dir = TempDir::new();
        let path = dir.path().join("note.txt").display().to_string();

        let created = write_file(
            None,
            WriteFileRequest {
                path: path.clone(),
                content: "hello".to_owned(),
            },
        )
        .await
        .unwrap();

        assert!(created.created);
        assert_eq!(created.metadata.size, 5);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");

        let replaced = write_file(
            None,
            WriteFileRequest {
                path,
                content: "hi".to_owned(),
            },
        )
        .await
        .unwrap();

        assert!(!replaced.created);
        assert_eq!(replaced.metadata.size, 2);
    }

    #[tokio::test]
    async fn creates_a_symlink_and_reports_conflicts() {
        let dir = TempDir::new();
        let path = dir.path().join("link").display().to_string();

        let described = create_symlink(
            None,
            CreateSymlinkRequest {
                path: path.clone(),
                target: "note.txt".to_owned(),
            },
        )
        .await
        .unwrap();

        assert_eq!(described.kind, ResourceKind::Symlink);

        let error = create_symlink(
            None,
            CreateSymlinkRequest {
                path,
                target: "other.txt".to_owned(),
            },
        )
        .await
        .unwrap_err();

        assert!(matches!(error, FileError::AlreadyExists(_)));
    }

    #[tokio::test]
    async fn patches_mode_and_modified_time() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new();
        let path = dir.path().join("note.txt");
        std::fs::write(&path, "hi").unwrap();

        let described = patch_metadata(
            None,
            PatchMetadataRequest {
                path: path.display().to_string(),
                mode: Some("0600".to_owned()),
                modified_at: Some("2026-01-01T00:00:00Z".to_owned()),
            },
        )
        .await
        .unwrap();

        assert_eq!(described.mode.as_deref(), Some("0600"));
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(described.modified_at, "2026-01-01T00:00:00+00:00");
    }

    #[tokio::test]
    async fn truncates_a_file_and_grows_it_with_zeroes() {
        let dir = TempDir::new();
        let path = dir.path().join("note.txt");
        std::fs::write(&path, "hello").unwrap();

        let described = truncate(
            None,
            TruncateRequest {
                path: path.display().to_string(),
                length: 2,
            },
        )
        .await
        .unwrap();

        assert_eq!(described.size, 2);
        assert_eq!(std::fs::read(&path).unwrap(), b"he");
    }

    #[tokio::test]
    async fn deletes_files_and_reports_missing_without_force() {
        let dir = TempDir::new();
        let path = dir.path().join("note.txt");
        std::fs::write(&path, "hi").unwrap();

        delete(
            None,
            DeleteRequest {
                path: path.display().to_string(),
                recursive: None,
                force: None,
            },
        )
        .await
        .unwrap();

        assert!(!path.exists());

        let error = delete(
            None,
            DeleteRequest {
                path: path.display().to_string(),
                recursive: None,
                force: None,
            },
        )
        .await
        .unwrap_err();
        assert!(
            matches!(error, FileError::Io(ref source) if source.kind() == std::io::ErrorKind::NotFound)
        );

        // `force` turns the missing target into a success.
        delete(
            None,
            DeleteRequest {
                path: path.display().to_string(),
                recursive: None,
                force: Some(true),
            },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn deletes_a_directory_only_when_recursive() {
        let dir = TempDir::new();
        let nested = dir.path().join("nested");
        std::fs::create_dir(&nested).unwrap();

        let error = delete(
            None,
            DeleteRequest {
                path: nested.display().to_string(),
                recursive: None,
                force: None,
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(error, FileError::NotAFile(_)));

        delete(
            None,
            DeleteRequest {
                path: nested.display().to_string(),
                recursive: Some(true),
                force: None,
            },
        )
        .await
        .unwrap();
        assert!(!nested.exists());
    }
}
