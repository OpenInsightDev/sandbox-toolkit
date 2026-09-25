//! Glob search over a directory subtree.

use globset::{Glob, GlobBuilder, GlobSet, GlobSetBuilder};
use thiserror::Error;

use super::dir::{DirectoryError, checked_target, resource_entry};
use super::model::{DirectoryResponse, ResourceEntry, ResourceKind};
use crate::workspace::registry::TargetFile;

/// Entries a response carries when the request names no `limit`, and the ceiling
/// an explicit `limit` is clamped to.
const SERVER_LIMIT: usize = 1000;

#[derive(Debug, Error)]
pub(crate) enum GlobError {
    #[error("resource not found: {0}")]
    NotFound(String),
    #[error("target is not a directory: {0}")]
    NotDirectory(String),
    #[error("path escapes workspace: {0}")]
    OutsideWorkspace(String),
    #[error("invalid glob pattern: {0}")]
    InvalidPattern(String),
    #[error("failed to walk the directory: {0}")]
    Io(#[from] std::io::Error),
}

/// The patterns and window of one glob search.
pub(crate) struct GlobRequest {
    /// Pattern a hit must match, relative to the search root.
    pub(crate) pattern: String,
    /// Patterns that remove a hit, and prune a matching directory's subtree.
    pub(crate) exclude: Vec<String>,
    pub(crate) offset: usize,
    /// Absent means [`SERVER_LIMIT`].
    pub(crate) limit: Option<usize>,
}

/// Locate the resources under `target` whose relative path matches the request.
///
/// Directories are visited whether or not they match, so a match below a
/// non-matching directory is still found; symbolic links are reported but never
/// descended into. Hits come back in directory-walk order and are deliberately
/// left unsorted, so the walk can stop once the window is filled.
pub(crate) async fn search(
    target: &TargetFile,
    request: &GlobRequest,
) -> Result<DirectoryResponse, GlobError> {
    let root = checked_target(target).await?;
    let matchers = Matchers::compile(&request.pattern, &request.exclude)?;
    let limit = request.limit.unwrap_or(SERVER_LIMIT).min(SERVER_LIMIT);

    let mut entries: Vec<ResourceEntry> = Vec::new();
    let mut skipped = 0;
    let mut truncated = false;

    let mut pending = vec![root.clone()];
    'walk: while let Some(directory) = pending.pop() {
        let mut read_dir = tokio::fs::read_dir(&directory).await?;
        while let Some(entry) = read_dir.next_entry().await? {
            let (resource, path) = resource_entry(entry, target).await?;
            let relative = path
                .strip_prefix(&root)
                .expect("a walked entry is always below the search root")
                .to_owned();
            if matchers.exclude.is_match(&relative) {
                continue;
            }
            if resource.kind == ResourceKind::Directory {
                pending.push(path);
            }
            if !matchers.include.is_match(&relative) {
                continue;
            }
            if skipped < request.offset {
                skipped += 1;
            } else if entries.len() < limit {
                entries.push(resource);
            } else {
                // One more hit sits past the window, so the response is short.
                truncated = true;
                break 'walk;
            }
        }
    }

    Ok(DirectoryResponse {
        path: root.display().to_string(),
        entries,
        truncated,
    })
}

/// The compiled include and exclude patterns of a search.
struct Matchers {
    include: GlobSet,
    exclude: GlobSet,
}

impl Matchers {
    fn compile(pattern: &str, exclude: &[String]) -> Result<Self, GlobError> {
        let mut include = GlobSetBuilder::new();
        include.add(glob(pattern)?);

        let mut excluded = GlobSetBuilder::new();
        for pattern in exclude {
            excluded.add(glob(pattern)?);
        }

        Ok(Self {
            include: build(include)?,
            exclude: build(excluded)?,
        })
    }
}

fn build(builder: GlobSetBuilder) -> Result<GlobSet, GlobError> {
    builder
        .build()
        .map_err(|error| GlobError::InvalidPattern(error.to_string()))
}

/// Compile one pattern with a literal separator, so `*` and `?` match within a
/// path component and only `**` crosses a `/`.
fn glob(pattern: &str) -> Result<Glob, GlobError> {
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map_err(|error| GlobError::InvalidPattern(error.to_string()))
}

impl From<DirectoryError> for GlobError {
    fn from(error: DirectoryError) -> Self {
        match error {
            DirectoryError::NotFound(path) => Self::NotFound(path),
            DirectoryError::NotDirectory(path) => Self::NotDirectory(path),
            DirectoryError::OutsideWorkspace(path) => Self::OutsideWorkspace(path),
            // Neither can arise while walking an existing, confined root.
            DirectoryError::AlreadyExists(_) | DirectoryError::ParentNotFound(_) => {
                Self::Io(std::io::Error::other(error.to_string()))
            }
            DirectoryError::Io(error) => Self::Io(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::registry::test_support::TempDir;

    fn request(pattern: &str) -> GlobRequest {
        GlobRequest {
            pattern: pattern.to_owned(),
            exclude: Vec::new(),
            offset: 0,
            limit: None,
        }
    }

    fn names(response: &DirectoryResponse) -> Vec<&str> {
        response
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect()
    }

    /// Hits arrive in walk order, which the filesystem does not fix, so tests
    /// that assert on the whole set sort the names first.
    fn sorted_names(response: &DirectoryResponse) -> Vec<&str> {
        let mut names = names(response);
        names.sort_unstable();
        names
    }

    #[tokio::test]
    async fn matches_recursively() {
        let dir = TempDir::new();
        tokio::fs::create_dir_all(dir.path().join("nested/deep"))
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("a.txt"), "a")
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("b.md"), "b")
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("nested/c.txt"), "c")
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("nested/deep/d.txt"), "d")
            .await
            .unwrap();
        let target = TargetFile::Absolute(dir.path().to_owned());

        let response = search(&target, &request("**/*.txt")).await.unwrap();

        assert_eq!(sorted_names(&response), ["a.txt", "c.txt", "d.txt"]);
        assert!(!response.truncated);
    }

    #[tokio::test]
    async fn a_single_star_stays_within_one_component() {
        let dir = TempDir::new();
        tokio::fs::create_dir(dir.path().join("nested"))
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("a.txt"), "a")
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("nested/c.txt"), "c")
            .await
            .unwrap();
        let target = TargetFile::Absolute(dir.path().to_owned());

        let response = search(&target, &request("*.txt")).await.unwrap();

        assert_eq!(names(&response), ["a.txt"]);
    }

    #[tokio::test]
    async fn matches_directories_and_symlinks_without_following_links() {
        let dir = TempDir::new();
        tokio::fs::create_dir(dir.path().join("nested"))
            .await
            .unwrap();
        tokio::fs::symlink(dir.path(), dir.path().join("loop"))
            .await
            .unwrap();
        let target = TargetFile::Absolute(dir.path().to_owned());

        let response = search(&target, &request("*")).await.unwrap();

        assert_eq!(sorted_names(&response), ["loop", "nested"]);
        let loop_entry = response
            .entries
            .iter()
            .find(|entry| entry.name == "loop")
            .unwrap();
        assert_eq!(loop_entry.kind, ResourceKind::Symlink);
    }

    #[tokio::test]
    async fn exclude_prunes_a_subtree_and_drops_other_hits() {
        let dir = TempDir::new();
        tokio::fs::create_dir(dir.path().join("nested"))
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("a.txt"), "a")
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("b.md"), "b")
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("nested/c.txt"), "c")
            .await
            .unwrap();
        let target = TargetFile::Absolute(dir.path().to_owned());

        let mut request = request("**/*");
        request.exclude = vec!["nested".to_owned(), "*.md".to_owned()];
        let response = search(&target, &request).await.unwrap();

        assert_eq!(names(&response), ["a.txt"]);
    }

    #[tokio::test]
    async fn paginates_a_window_of_the_walk() {
        let dir = TempDir::new();
        for name in ["a.txt", "b.txt", "c.txt"] {
            tokio::fs::write(dir.path().join(name), name).await.unwrap();
        }
        let target = TargetFile::Absolute(dir.path().to_owned());

        let mut window = request("*.txt");
        window.limit = Some(2);
        let response = search(&target, &window).await.unwrap();
        assert_eq!(response.entries.len(), 2);
        assert!(response.truncated);

        let mut trimmed = request("*.txt");
        trimmed.offset = 3;
        let response = search(&target, &trimmed).await.unwrap();
        assert!(response.entries.is_empty());
        assert!(!response.truncated);
    }

    #[tokio::test]
    async fn rejects_a_file_a_missing_path_and_a_bad_pattern() {
        let dir = TempDir::new();
        tokio::fs::write(dir.path().join("a.txt"), "a")
            .await
            .unwrap();

        assert!(matches!(
            search(
                &TargetFile::Absolute(dir.path().join("a.txt")),
                &request("*")
            )
            .await,
            Err(GlobError::NotDirectory(_))
        ));
        assert!(matches!(
            search(
                &TargetFile::Absolute(dir.path().join("missing")),
                &request("*")
            )
            .await,
            Err(GlobError::NotFound(_))
        ));
        assert!(matches!(
            search(&TargetFile::Absolute(dir.path().to_owned()), &request("[")).await,
            Err(GlobError::InvalidPattern(_))
        ));
    }
}
