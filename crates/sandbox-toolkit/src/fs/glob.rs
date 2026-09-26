use globset::{Glob, GlobBuilder, GlobSet, GlobSetBuilder};

use super::dir::{DirectoryError, SERVER_LIMIT, checked_target, resource_entry};
use super::model::{DirectoryResponse, GlobRequest, ResourceKind};
use crate::workspace::registry::Workspace;

/// Directories are visited whether or not they match, so a match below a
/// non-matching directory is still found; symbolic links are reported but never
/// descended into. Hits come back in directory-walk order and are deliberately
/// left unsorted, so the walk can stop once the window is filled.
pub(crate) async fn search(
    workspace: Option<&Workspace>,
    request: &GlobRequest,
) -> Result<DirectoryResponse, DirectoryError> {
    let root = checked_target(workspace, &request.path).await?;
    let matchers = Matchers::compile(&request.pattern, &request.exclude)?;
    let limit = request.limit.unwrap_or(SERVER_LIMIT).min(SERVER_LIMIT) as usize;

    let mut entries = Vec::new();
    let mut skipped = 0;
    let mut truncated = false;

    let mut pending = vec![root.clone()];
    'walk: while let Some(directory) = pending.pop() {
        let mut read_dir = tokio::fs::read_dir(&directory).await?;
        while let Some(entry) = read_dir.next_entry().await? {
            let path = entry.path();
            let resource = resource_entry(&entry, &root, &request.path).await?;
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
            if skipped < request.offset.unwrap_or(0) {
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

    Ok(DirectoryResponse { entries, truncated })
}

struct Matchers {
    include: GlobSet,
    exclude: GlobSet,
}

impl Matchers {
    fn compile(pattern: &str, exclude: &[String]) -> Result<Self, DirectoryError> {
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

fn build(builder: GlobSetBuilder) -> Result<GlobSet, DirectoryError> {
    builder
        .build()
        .map_err(|error| DirectoryError::InvalidPattern(error.to_string()))
}

/// Compile one pattern with a literal separator, so `*` and `?` match within a
/// path component and only `**` crosses a `/`.
fn glob(pattern: &str) -> Result<Glob, DirectoryError> {
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map_err(|error| DirectoryError::InvalidPattern(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::registry::test_support::TempDir;

    fn request(path: &str, pattern: &str) -> GlobRequest {
        GlobRequest {
            path: path.to_owned(),
            pattern: pattern.to_owned(),
            exclude: Vec::new(),
            offset: None,
            limit: None,
        }
    }

    fn names(response: &DirectoryResponse) -> Vec<String> {
        let mut names: Vec<String> = response
            .entries
            .iter()
            .map(|entry| entry.name.clone())
            .collect();
        names.sort_unstable();
        names
    }

    fn address(dir: &TempDir) -> String {
        dir.path().display().to_string()
    }

    #[tokio::test]
    async fn matches_recursively_and_stays_within_one_component() {
        let dir = TempDir::new();
        std::fs::create_dir_all(dir.path().join("nested/deep")).unwrap();
        std::fs::write(dir.path().join("a.txt"), "a").unwrap();
        std::fs::write(dir.path().join("nested/c.txt"), "c").unwrap();
        std::fs::write(dir.path().join("nested/deep/d.txt"), "d").unwrap();

        let recursive = search(None, &request(&address(&dir), "**/*.txt"))
            .await
            .unwrap();
        assert_eq!(names(&recursive), ["a.txt", "c.txt", "d.txt"]);

        let shallow = search(None, &request(&address(&dir), "*.txt"))
            .await
            .unwrap();
        assert_eq!(names(&shallow), ["a.txt"]);
    }

    #[tokio::test]
    async fn exclude_prunes_a_subtree_and_drops_other_hits() {
        let dir = TempDir::new();
        std::fs::create_dir(dir.path().join("nested")).unwrap();
        std::fs::write(dir.path().join("a.txt"), "a").unwrap();
        std::fs::write(dir.path().join("b.md"), "b").unwrap();
        std::fs::write(dir.path().join("nested/c.txt"), "c").unwrap();

        let mut request = request(&address(&dir), "**/*");
        request.exclude = vec!["nested".to_owned(), "*.md".to_owned()];
        let response = search(None, &request).await.unwrap();

        assert_eq!(names(&response), ["a.txt"]);
    }

    #[tokio::test]
    async fn rejects_a_file_root_and_a_bad_pattern() {
        let dir = TempDir::new();
        std::fs::write(dir.path().join("a.txt"), "a").unwrap();

        assert!(matches!(
            search(None, &request(&address(&dir), "[")).await,
            Err(DirectoryError::InvalidPattern(_))
        ));

        let file = dir.path().join("a.txt").display().to_string();
        assert!(matches!(
            search(None, &request(&file, "*")).await,
            Err(DirectoryError::NotDirectory(_))
        ));
    }
}
