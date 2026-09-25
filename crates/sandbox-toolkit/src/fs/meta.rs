//! The facts one resource reports about itself.

use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use thiserror::Error;

use super::model::{ResourceKind, ResourceMetadata};
use crate::workspace::registry::TargetFile;

#[derive(Debug, Error)]
pub(crate) enum MetadataError {
    #[error("resource not found: {0}")]
    NotFound(String),
    #[error("path escapes workspace: {0}")]
    OutsideWorkspace(String),
    #[error("failed to read resource metadata: {0}")]
    Io(#[from] std::io::Error),
}

/// Describe the target as it stands: a trailing symlink is reported as a
/// symlink and never followed to what it points at.
pub(super) async fn read_metadata(target: &TargetFile) -> Result<ResourceMetadata, MetadataError> {
    let path = target.path();
    let metadata = tokio::fs::symlink_metadata(&path).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            MetadataError::NotFound(path.display().to_string())
        } else {
            MetadataError::Io(error)
        }
    })?;

    let link_target = if metadata.file_type().is_symlink() {
        Some(tokio::fs::read_link(&path).await?)
    } else {
        None
    };

    confine(target, &path, link_target.as_deref()).await?;
    let platform = platform_attributes(&metadata);

    Ok(ResourceMetadata {
        name: path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        path: target.address(),
        kind: kind(&metadata),
        // Only a file has content for the data plane to carry.
        size: if metadata.is_file() {
            metadata.len()
        } else {
            0
        },
        etag: etag(&metadata),
        mode: platform.mode,
        uid: platform.uid,
        gid: platform.gid,
        inode: platform.inode,
        links: platform.links,
        device: platform.device,
        device_type: platform.device_type,
        block_size: platform.block_size,
        blocks: platform.blocks,
        modified_at: modified_at(&metadata),
        accessed_at: format_time(metadata.accessed().ok()),
        birthtime: format_time(metadata.created().ok()),
        target: link_target.map(|target| target.to_string_lossy().into_owned()),
    })
}

/// The attributes only some platforms record, so a resource can omit what its
/// host cannot report.
#[derive(Debug, Default)]
struct PlatformAttributes {
    mode: Option<String>,
    uid: Option<u64>,
    gid: Option<u64>,
    inode: Option<u64>,
    links: Option<u64>,
    device: Option<u64>,
    device_type: Option<u64>,
    block_size: Option<u64>,
    blocks: Option<u64>,
}

/// Read the attributes `stat(2)` exposes through the Unix metadata extension.
#[cfg(unix)]
fn platform_attributes(metadata: &std::fs::Metadata) -> PlatformAttributes {
    use std::os::unix::fs::MetadataExt;

    PlatformAttributes {
        // The permission bits only, without the file-type bits `mode` also carries.
        mode: Some(format!("{:04o}", metadata.mode() & 0o7777)),
        uid: Some(metadata.uid().into()),
        gid: Some(metadata.gid().into()),
        inode: Some(metadata.ino()),
        links: Some(metadata.nlink()),
        device: Some(metadata.dev()),
        device_type: Some(metadata.rdev()),
        block_size: Some(metadata.blksize()),
        blocks: Some(metadata.blocks()),
    }
}

/// Platforms without the Unix metadata extension report none of these fields.
#[cfg(not(unix))]
fn platform_attributes(_metadata: &std::fs::Metadata) -> PlatformAttributes {
    PlatformAttributes::default()
}

/// A filesystem timestamp as RFC 3339 at whole-second precision, or `None` when
/// the platform or filesystem does not record it.
fn format_time(time: Option<SystemTime>) -> Option<String> {
    let duration = time?.duration_since(UNIX_EPOCH).ok()?;
    chrono::DateTime::<chrono::Utc>::from_timestamp(duration.as_secs() as i64, 0)
        .map(|time| time.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

fn kind(metadata: &std::fs::Metadata) -> ResourceKind {
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        ResourceKind::Symlink
    } else if file_type.is_dir() {
        ResourceKind::Directory
    } else {
        ResourceKind::File
    }
}

/// Confine a workspace target to its root.
///
/// The trailing symlink is kept as-is, but where it points still decides
/// containment. A dangling link resolves to nothing, so its target is judged
/// lexically instead.
async fn confine(
    target: &TargetFile,
    path: &Path,
    link_target: Option<&Path>,
) -> Result<(), MetadataError> {
    let Some(root) = target.workspace_root() else {
        return Ok(());
    };
    let root = tokio::fs::canonicalize(root).await?;

    let resolved = match tokio::fs::canonicalize(path).await {
        Ok(resolved) => resolved,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let link_target =
                link_target.ok_or_else(|| MetadataError::NotFound(path.display().to_string()))?;
            dangling_target(path, link_target).await?
        }
        Err(error) => return Err(error.into()),
    };

    if resolved.starts_with(&root) {
        Ok(())
    } else {
        Err(MetadataError::OutsideWorkspace(path.display().to_string()))
    }
}

/// Where a dangling symlink points, touching nothing past its own directory.
async fn dangling_target(path: &Path, link_target: &Path) -> Result<PathBuf, MetadataError> {
    let Some(parent) = path.parent() else {
        return Ok(link_target.to_owned());
    };
    let joined = if link_target.is_absolute() {
        link_target.to_owned()
    } else {
        tokio::fs::canonicalize(parent).await?.join(link_target)
    };

    Ok(lexical_normalize(&joined))
}

/// Resolve `.` and `..` without touching the filesystem, so a path that does
/// not exist can still be compared against the workspace root.
fn lexical_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            component => normalized.push(component.as_os_str()),
        }
    }
    normalized
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

/// Modification time as RFC 3339, at the whole-second precision the
/// `Last-Modified` header of the same resource carries.
pub(super) fn modified_at(metadata: &std::fs::Metadata) -> String {
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok());

    match modified {
        Some(duration) => {
            chrono::DateTime::<chrono::Utc>::from_timestamp(duration.as_secs() as i64, 0)
                .expect("a filesystem timestamp is within the representable range")
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        }
        None => "1970-01-01T00:00:00Z".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::workspace::model::WorkspaceProperties;
    use crate::workspace::registry::{WorkspaceRegistry, test_support::TempDir};

    fn absolute(path: PathBuf) -> TargetFile {
        TargetFile::Absolute(path)
    }

    #[tokio::test]
    async fn describes_a_file_a_directory_and_a_symlink() {
        let dir = TempDir::new();
        tokio::fs::write(dir.path().join("note.txt"), "hello")
            .await
            .unwrap();
        tokio::fs::create_dir(dir.path().join("notes"))
            .await
            .unwrap();
        tokio::fs::symlink("note.txt", dir.path().join("link"))
            .await
            .unwrap();

        let file = read_metadata(&absolute(dir.path().join("note.txt")))
            .await
            .unwrap();
        assert_eq!(file.name, "note.txt");
        assert_eq!(file.kind, ResourceKind::File);
        assert_eq!(file.size, 5);
        assert!(file.target.is_none());
        assert!(file.etag.starts_with('"'));

        let directory = read_metadata(&absolute(dir.path().join("notes")))
            .await
            .unwrap();
        assert_eq!(directory.kind, ResourceKind::Directory);
        assert_eq!(directory.size, 0);

        let link = read_metadata(&absolute(dir.path().join("link")))
            .await
            .unwrap();
        assert_eq!(link.kind, ResourceKind::Symlink);
        assert_eq!(link.size, 0);
        assert_eq!(link.target.as_deref(), Some("note.txt"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reports_the_platform_attributes_of_a_file() {
        use std::os::unix::fs::MetadataExt;

        let dir = TempDir::new();
        let path = dir.path().join("note.txt");
        tokio::fs::write(&path, "hello").await.unwrap();

        let metadata = read_metadata(&absolute(path.clone())).await.unwrap();
        let raw = std::fs::metadata(&path).unwrap();

        let expected_mode = format!("{:04o}", raw.mode() & 0o7777);
        assert_eq!(metadata.mode.as_deref(), Some(expected_mode.as_str()));
        assert_eq!(metadata.uid, Some(raw.uid().into()));
        assert_eq!(metadata.gid, Some(raw.gid().into()));
        assert_eq!(metadata.inode, Some(raw.ino()));
        assert_eq!(metadata.links, Some(raw.nlink()));
        assert_eq!(metadata.device, Some(raw.dev()));
        assert!(metadata.block_size.is_some());
        assert!(metadata.blocks.is_some());

        assert!(
            chrono::DateTime::parse_from_rfc3339(metadata.accessed_at.as_deref().unwrap()).is_ok()
        );
        if let Some(birthtime) = &metadata.birthtime {
            assert!(chrono::DateTime::parse_from_rfc3339(birthtime).is_ok());
        }
    }

    #[tokio::test]
    async fn reports_zero_size_and_the_target_of_a_dangling_symlink() {
        let dir = TempDir::new();
        tokio::fs::symlink("missing.txt", dir.path().join("ghost"))
            .await
            .unwrap();

        let ghost = read_metadata(&absolute(dir.path().join("ghost")))
            .await
            .unwrap();

        assert_eq!(ghost.kind, ResourceKind::Symlink);
        assert_eq!(ghost.size, 0);
        assert_eq!(ghost.target.as_deref(), Some("missing.txt"));
    }

    #[tokio::test]
    async fn reports_the_path_in_the_addressing_mode() {
        let dir = TempDir::new();
        tokio::fs::write(dir.path().join("note.txt"), "hello")
            .await
            .unwrap();
        let registry = WorkspaceRegistry::default();
        registry
            .register("docs", &dir.root(), WorkspaceProperties::default())
            .await
            .unwrap();

        let workspace = registry
            .target_file("docs", PathBuf::from("note.txt"))
            .unwrap();
        let metadata = read_metadata(&workspace).await.unwrap();
        assert_eq!(metadata.path, "note.txt");
        assert_eq!(metadata.name, "note.txt");

        let root = registry.target_file("docs", PathBuf::new()).unwrap();
        let root = read_metadata(&root).await.unwrap();
        assert_eq!(root.path, "");
        assert_eq!(root.name, canonical_root_name(&dir));

        let remote = read_metadata(&absolute(dir.path().join("note.txt")))
            .await
            .unwrap();
        assert_eq!(
            remote.path,
            dir.path().join("note.txt").display().to_string()
        );
    }

    /// Name the registry stores the workspace root under.
    fn canonical_root_name(dir: &TempDir) -> String {
        std::fs::canonicalize(dir.path())
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned()
    }

    #[tokio::test]
    async fn formats_the_modification_time_as_rfc3339() {
        let dir = TempDir::new();
        tokio::fs::write(dir.path().join("note.txt"), "hello")
            .await
            .unwrap();

        let metadata = read_metadata(&absolute(dir.path().join("note.txt")))
            .await
            .unwrap();
        let reported = chrono::DateTime::parse_from_rfc3339(&metadata.modified_at).unwrap();

        let modified = tokio::fs::metadata(dir.path().join("note.txt"))
            .await
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert_eq!(reported.timestamp(), modified);
    }

    #[tokio::test]
    async fn confines_workspace_targets() {
        let dir = TempDir::new();
        tokio::fs::write(dir.path().join("note.txt"), "hello")
            .await
            .unwrap();
        tokio::fs::symlink("..", dir.path().join("escape"))
            .await
            .unwrap();
        tokio::fs::symlink("../nope", dir.path().join("dangling-escape"))
            .await
            .unwrap();
        let registry = WorkspaceRegistry::default();
        registry
            .register("docs", &dir.root(), WorkspaceProperties::default())
            .await
            .unwrap();

        for path in ["escape", "dangling-escape"] {
            let target = registry.target_file("docs", PathBuf::from(path)).unwrap();
            assert!(
                matches!(
                    read_metadata(&target).await,
                    Err(MetadataError::OutsideWorkspace(_))
                ),
                "`{path}` was not rejected"
            );
        }

        let inside = registry
            .target_file("docs", PathBuf::from("note.txt"))
            .unwrap();
        assert_eq!(read_metadata(&inside).await.unwrap().name, "note.txt");
    }

    #[tokio::test]
    async fn reports_missing_resources() {
        let dir = TempDir::new();

        let error = read_metadata(&absolute(dir.path().join("missing")))
            .await
            .unwrap_err();

        assert!(matches!(error, MetadataError::NotFound(_)));
    }

    #[test]
    fn normalizes_paths_lexically() {
        assert_eq!(
            lexical_normalize(Path::new("/srv/project/../a/./b.txt")),
            PathBuf::from("/srv/a/b.txt")
        );
        assert_eq!(lexical_normalize(Path::new("/../x")), PathBuf::from("/x"));
    }
}
