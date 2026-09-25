//! Path validation and workspace confinement. An address is percent-decoded
//! exactly once and accepted only in normalized form, never corrected, so the
//! resource operations can assume a normalized path.

use std::path::{Component, Path, PathBuf};

use percent_encoding::percent_decode_str;
use thiserror::Error;

use super::TargetFile;

#[derive(Debug, Error)]
pub(crate) enum PathError {
    #[error("path must not contain NUL")]
    Nul,
    #[error("path must be relative")]
    NotRelative,
    #[error("path must be normalized")]
    NotNormalized,
    #[error("path contains an ambiguous encoding")]
    AmbiguousEncoding,
    #[error("resource not found: {0}")]
    NotFound(String),
    #[error("path escapes workspace: {0}")]
    OutsideWorkspace(String),
    #[error("failed to resolve path: {0}")]
    Io(#[from] std::io::Error),
}

/// Validate a workspace-relative address. The empty address is the root.
pub(crate) fn validate_relative(raw: &str) -> Result<PathBuf, PathError> {
    let decoded = decode(raw)?;
    if decoded.starts_with('/') {
        return Err(PathError::NotRelative);
    }

    Ok(components(&decoded)?.into_iter().collect())
}

/// Validate a remote absolute address. `raw` is the `/fs/` suffix without its
/// leading separator, so the empty address is the filesystem root.
pub(crate) fn validate_absolute(raw: &str) -> Result<PathBuf, PathError> {
    let decoded = decode(raw)?;
    if decoded.starts_with('/') {
        return Err(PathError::NotNormalized);
    }

    let mut path = PathBuf::from("/");
    path.extend(components(&decoded)?);

    Ok(path)
}

/// Percent-decode the raw capture once. An escape for a path separator is
/// rejected: whether a byte separates components must not depend on where
/// decoding happens.
fn decode(raw: &str) -> Result<String, PathError> {
    let bytes = raw.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            index += 1;
            continue;
        }

        let high = bytes.get(index + 1).and_then(|byte| hex(*byte));
        let low = bytes.get(index + 2).and_then(|byte| hex(*byte));
        let decoded = high
            .zip(low)
            .map(|(high, low)| high << 4 | low)
            .ok_or(PathError::AmbiguousEncoding)?;
        if matches!(decoded, b'/' | b'\\' | 0) {
            return Err(PathError::AmbiguousEncoding);
        }
        index += 3;
    }

    percent_decode_str(raw)
        .decode_utf8()
        .map(|decoded| decoded.into_owned())
        .map_err(|_| PathError::AmbiguousEncoding)
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Split a decoded address into its components. `.`, `..`, empty segments, NUL
/// and backslashes are rejected rather than removed.
fn components(decoded: &str) -> Result<Vec<&str>, PathError> {
    if decoded.contains('\0') {
        return Err(PathError::Nul);
    }
    if decoded.contains('\\') {
        return Err(PathError::AmbiguousEncoding);
    }
    if decoded.is_empty() {
        return Ok(Vec::new());
    }

    decoded
        .split('/')
        .map(|component| match component {
            "" | "." | ".." => Err(PathError::NotNormalized),
            component => Ok(component),
        })
        .collect()
}

/// Resolve an existing target, following any symbolic link. The returned path is
/// absolute and, in workspace mode, confined to the workspace root.
pub(crate) async fn resolve_existing(target: &TargetFile) -> Result<PathBuf, PathError> {
    let path = target.path();
    let resolved = tokio::fs::canonicalize(&path).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            PathError::NotFound(path.display().to_string())
        } else {
            PathError::Io(error)
        }
    })?;

    match target.workspace_root() {
        Some(root) => {
            let root = tokio::fs::canonicalize(root).await?;
            confine(root, resolved, &path)
        }
        None => Ok(resolved),
    }
}

/// Resolve a target whose trailing components may not exist yet. The deepest
/// existing ancestor is canonicalized and the literal remainder appended, so a
/// creation target is confined before anything is created.
pub(crate) async fn resolve_creating(target: &TargetFile) -> Result<PathBuf, PathError> {
    let path = target.path();
    let Some(root) = target.workspace_root() else {
        return Ok(path);
    };
    let root = tokio::fs::canonicalize(root).await?;

    // A request path is already normalized, so the literal remainder needs no
    // further resolution.
    let (existing, resolved) = deepest_existing(&path).await?;
    let remaining = path.strip_prefix(existing).unwrap_or(Path::new(""));

    confine(root, resolved.join(remaining), &path)
}

/// Resolve a target that is itself a symbolic link, without following its final
/// link. A dangling link is judged by resolving `link_target` up to the deepest
/// existing ancestor, so a link that points outside the workspace is still
/// rejected.
pub(crate) async fn resolve_link(
    target: &TargetFile,
    link_target: &Path,
) -> Result<PathBuf, PathError> {
    let path = target.path();
    let Some(root) = target.workspace_root() else {
        return Ok(path);
    };
    let root = tokio::fs::canonicalize(root).await?;

    let resolved = match tokio::fs::canonicalize(&path).await {
        Ok(resolved) => resolved,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            dangling_target(&path, link_target).await?
        }
        Err(error) => return Err(error.into()),
    };

    confine(root, resolved, &path)
}

fn confine(root: PathBuf, resolved: PathBuf, path: &Path) -> Result<PathBuf, PathError> {
    if resolved.starts_with(&root) {
        Ok(resolved)
    } else {
        Err(PathError::OutsideWorkspace(path.display().to_string()))
    }
}

async fn deepest_existing(path: &Path) -> Result<(&Path, PathBuf), PathError> {
    let mut current = path;
    loop {
        match tokio::fs::canonicalize(current).await {
            Ok(resolved) => return Ok((current, resolved)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                current = current
                    .parent()
                    .ok_or_else(|| PathError::OutsideWorkspace(path.display().to_string()))?;
            }
            Err(error) => return Err(error.into()),
        }
    }
}

async fn dangling_target(path: &Path, link_target: &Path) -> Result<PathBuf, PathError> {
    let parent = path
        .parent()
        .ok_or_else(|| PathError::OutsideWorkspace(path.display().to_string()))?;
    let joined = if link_target.is_absolute() {
        link_target.to_owned()
    } else {
        tokio::fs::canonicalize(parent).await?.join(link_target)
    };

    // The filesystem resolves the target up to its deepest existing ancestor; a
    // `..` past that point would land somewhere the link's future target could
    // still escape to, so it is refused rather than guessed at.
    let (existing, resolved) = deepest_existing(&joined).await?;
    let remaining = joined.strip_prefix(existing).unwrap_or(Path::new(""));
    if remaining
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(PathError::OutsideWorkspace(path.display().to_string()));
    }

    Ok(resolved.join(remaining))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::workspace::model::WorkspaceProperties;
    use crate::workspace::registry::{WorkspaceRegistry, test_support::TempDir};

    #[test]
    fn accepts_normalized_relative_addresses() {
        assert_eq!(validate_relative("").unwrap(), PathBuf::new());
        assert_eq!(
            validate_relative("a/b.txt").unwrap(),
            PathBuf::from("a/b.txt")
        );
        assert_eq!(
            validate_relative("a%20b/c").unwrap(),
            PathBuf::from("a b/c"),
            "a percent escape outside the separators still decodes once"
        );
    }

    #[test]
    fn rejects_a_relative_address_that_is_not_normalized() {
        for raw in [
            "/a", ".", "..", "a/./b", "a/../b", "a//b", "a/", "%2e%2e", "a%00b",
        ] {
            assert!(validate_relative(raw).is_err(), "`{raw}` was accepted");
        }
    }

    #[test]
    fn rejects_an_ambiguous_encoding() {
        for raw in ["a%2Fb", "a%2fb", "%2e%2e%2f", "a%5Cb", "%"] {
            assert!(
                matches!(validate_relative(raw), Err(PathError::AmbiguousEncoding)),
                "`{raw}` was not ambiguous"
            );
        }
    }

    #[test]
    fn accepts_normalized_absolute_addresses() {
        assert_eq!(validate_absolute("").unwrap(), PathBuf::from("/"));
        assert_eq!(
            validate_absolute("srv/project/a.txt").unwrap(),
            PathBuf::from("/srv/project/a.txt")
        );
    }

    #[test]
    fn rejects_an_absolute_address_that_is_not_normalized() {
        for raw in ["/srv", "srv/../etc", "srv//a", "srv/"] {
            assert!(validate_absolute(raw).is_err(), "`{raw}` was accepted");
        }
    }

    #[tokio::test]
    async fn confines_an_existing_workspace_target() {
        let dir = TempDir::new();
        tokio::fs::write(dir.path().join("note.txt"), "hello")
            .await
            .unwrap();
        let registry = WorkspaceRegistry::default();
        registry
            .register("docs", &dir.root(), WorkspaceProperties::default())
            .await
            .unwrap();

        let target = TargetFile::workspace(&registry, "docs", PathBuf::from("note.txt")).unwrap();
        assert_eq!(
            resolve_existing(&target).await.unwrap(),
            tokio::fs::canonicalize(dir.path().join("note.txt"))
                .await
                .unwrap()
        );

        let missing = TargetFile::workspace(&registry, "docs", PathBuf::from("missing")).unwrap();
        assert!(matches!(
            resolve_existing(&missing).await,
            Err(PathError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn confines_a_creation_target_below_a_missing_parent() {
        let dir = TempDir::new();
        let registry = WorkspaceRegistry::default();
        registry
            .register("docs", &dir.root(), WorkspaceProperties::default())
            .await
            .unwrap();

        let target = TargetFile::workspace(&registry, "docs", PathBuf::from("a/b/c")).unwrap();

        assert_eq!(
            resolve_creating(&target).await.unwrap(),
            tokio::fs::canonicalize(dir.path())
                .await
                .unwrap()
                .join("a/b/c")
        );
    }

    #[tokio::test]
    async fn confines_dangling_links_by_their_target() {
        let dir = TempDir::new();
        let registry = WorkspaceRegistry::default();
        registry
            .register("docs", &dir.root(), WorkspaceProperties::default())
            .await
            .unwrap();

        let inside = TargetFile::workspace(&registry, "docs", PathBuf::from("inside")).unwrap();
        assert!(
            resolve_link(&inside, Path::new("missing.txt"))
                .await
                .is_ok()
        );

        // `..` inside the existing prefix is resolved by the filesystem; `..`
        // past a missing component cannot be, so an escape cannot be ruled out.
        let outside = TargetFile::workspace(&registry, "docs", PathBuf::from("outside")).unwrap();
        assert!(matches!(
            resolve_link(&outside, Path::new("../nope")).await,
            Err(PathError::OutsideWorkspace(_))
        ));
        assert!(matches!(
            resolve_link(&outside, Path::new("missing/../../nope")).await,
            Err(PathError::OutsideWorkspace(_))
        ));
    }

    #[tokio::test]
    async fn absolute_targets_have_no_boundary() {
        let target = TargetFile::Absolute(PathBuf::from("/srv/project/a.txt"));
        assert_eq!(
            resolve_creating(&target).await.unwrap(),
            PathBuf::from("/srv/project/a.txt")
        );
        assert_eq!(
            resolve_link(&target, Path::new("../../etc")).await.unwrap(),
            PathBuf::from("/srv/project/a.txt")
        );
    }
}
