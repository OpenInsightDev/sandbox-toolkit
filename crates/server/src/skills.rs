//! Agent Skills (`skills/*`) — the skills a user installs under
//! `~/.agents/skills`, one directory per skill holding a `SKILL.md` whose
//! `---`-fenced frontmatter and Markdown body follow the Agent Skills
//! specification.

use std::{
    io,
    path::{Path, PathBuf},
};

use agent_plugins::{SkillInvalid, SkillMeta, parse_skill_md};
use thiserror::Error;

use crate::model::{GetSkillParams, GetSkillResult, ListSkillsResult, Skill};

/// Filename a skill's instructions are read from, inside its directory.
const SKILL_FILE: &str = "SKILL.md";

/// Directory, relative to the user's home, that skills are discovered under.
const SKILLS_DIR: &str = ".agents/skills";

/// Why a skill could not be listed or read.
#[derive(Debug, Error)]
pub enum SkillsError {
    /// The user's home directory could not be determined.
    #[error("cannot locate the skills directory: HOME is not set")]
    NoHome,
    /// The requested skill name is not a single, plain directory name.
    #[error("invalid skill name: {0:?}")]
    InvalidName(String),
    /// No skill exists with the requested name.
    #[error("unknown skill: {0}")]
    Unknown(String),
    /// The skills directory could not be read.
    #[error("failed to read the skills directory {}", .0.display())]
    ReadDir(PathBuf, #[source] io::Error),
    /// A skill's `SKILL.md` could not be read.
    #[error("failed to read {}", .0.display())]
    ReadFile(PathBuf, #[source] io::Error),
    /// A skill's `SKILL.md` does not satisfy the Agent Skills specification.
    #[error("skill {0:?} is invalid: {1}")]
    Invalid(String, #[source] SkillInvalid),
}

/// The directory skills are discovered under: `$HOME/.agents/skills`.
fn root() -> Result<PathBuf, SkillsError> {
    let home = std::env::var_os("HOME").ok_or(SkillsError::NoHome)?;
    Ok(PathBuf::from(home).join(SKILLS_DIR))
}

/// List every valid skill under the user's skills directory, sorted by name.
pub async fn list() -> Result<ListSkillsResult, SkillsError> {
    list_at(&root()?).await
}

/// Read one skill by name, together with its Markdown instructions.
pub async fn get(params: &GetSkillParams) -> Result<GetSkillResult, SkillsError> {
    get_at(&root()?, &params.name).await
}

/// List the skills under an explicit directory. A missing directory is an
/// empty list; skills that cannot be read or fail validation are skipped.
async fn list_at(root: &Path) -> Result<ListSkillsResult, SkillsError> {
    let mut entries = match tokio::fs::read_dir(root).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(ListSkillsResult { skills: Vec::new() });
        }
        Err(error) => return Err(SkillsError::ReadDir(root.to_path_buf(), error)),
    };

    let mut skills = Vec::new();
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| SkillsError::ReadDir(root.to_path_buf(), error))?
    {
        if !is_directory(&entry).await {
            continue;
        }

        let Some(directory) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };

        if let Some(skill) = load(&entry.path(), &directory).await {
            skills.push(skill);
        }
    }

    skills.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(ListSkillsResult { skills })
}

/// Read the skill in `root/<name>`. Rejects names that are not a single path
/// component so a request cannot escape `root`.
async fn get_at(root: &Path, name: &str) -> Result<GetSkillResult, SkillsError> {
    if !is_plain_name(name) {
        return Err(SkillsError::InvalidName(name.to_owned()));
    }

    let directory = root.join(name);
    let path = directory.join(SKILL_FILE);
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|error| match error.kind() {
            io::ErrorKind::NotFound | io::ErrorKind::NotADirectory => {
                SkillsError::Unknown(name.to_owned())
            }
            _ => SkillsError::ReadFile(path.clone(), error),
        })?;

    let (meta, body) = parse_skill_md(name, &bytes)
        .map_err(|error| SkillsError::Invalid(name.to_owned(), error))?;

    Ok(GetSkillResult {
        skill: skill_from(meta, name, &directory),
        content: body.source().to_owned(),
    })
}

/// Read and validate the `SKILL.md` in `directory`, or `None` when it is
/// missing or invalid.
async fn load(directory: &Path, name: &str) -> Option<Skill> {
    let bytes = tokio::fs::read(directory.join(SKILL_FILE)).await.ok()?;
    let (meta, _) = parse_skill_md(name, &bytes).ok()?;
    Some(skill_from(meta, name, directory))
}

fn skill_from(meta: SkillMeta, directory: &str, path: &Path) -> Skill {
    Skill {
        name: meta.name,
        directory: directory.to_owned(),
        path: path.display().to_string(),
        description: meta.description,
        license: meta.license,
        compatibility: meta.compatibility,
        allowed_tools: meta.allowed_tools,
        metadata: meta.metadata,
    }
}

async fn is_directory(entry: &tokio::fs::DirEntry) -> bool {
    entry
        .file_type()
        .await
        .is_ok_and(|file_type| file_type.is_dir())
}

/// A name that is exactly one ordinary path component.
fn is_plain_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains('/') && !name.contains('\\')
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a unique, empty scratch directory for one test.
    fn scratch(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "sandbox-toolkit-skills-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("creating the scratch directory failed");
        path
    }

    /// Write a `SKILL.md` with the given frontmatter and body into `dir/<name>`.
    fn fixture(root: &Path, name: &str, frontmatter: &str, body: &str) {
        let directory = root.join(name);
        std::fs::create_dir_all(&directory).expect("creating the skill directory failed");
        std::fs::write(
            directory.join(SKILL_FILE),
            format!("---\n{frontmatter}\n---\n{body}"),
        )
        .expect("writing the SKILL.md failed");
    }

    const GREET: &str = "name: greet\ndescription: Greets. Use when greeting.";

    #[tokio::test]
    async fn lists_valid_skills_sorted_and_skips_the_rest() {
        let root = scratch("list");
        fixture(&root, "greet", GREET, "Say hi.\n");
        fixture(
            &root,
            "summarize",
            "name: summarize\ndescription: Summarizes.",
            "",
        );
        fixture(&root, "broken", "description: no name here", "");
        std::fs::create_dir_all(root.join("empty")).unwrap();
        std::fs::write(root.join("loose.txt"), "not a skill").unwrap();

        let result = list_at(&root).await.unwrap();

        let names: Vec<String> = result.skills.into_iter().map(|skill| skill.name).collect();
        assert_eq!(names, vec!["greet", "summarize"]);
    }

    #[tokio::test]
    async fn a_missing_directory_lists_nothing() {
        let root = scratch("missing");

        let result = list_at(&root.join("nope")).await.unwrap();

        assert!(result.skills.is_empty());
    }

    #[tokio::test]
    async fn reads_a_skill_with_its_body_and_optionals() {
        let root = scratch("get");
        fixture(
            &root,
            "greet",
            "name: greet\ndescription: Greets.\nlicense: MIT\nallowed-tools: read_file\nmetadata:\n  version: 1",
            "Say hi.\n",
        );

        let result = get_at(&root, "greet").await.unwrap();

        assert_eq!(result.skill.name, "greet");
        assert_eq!(result.skill.directory, "greet");
        assert_eq!(result.skill.path, root.join("greet").display().to_string());
        assert_eq!(result.skill.description, "Greets.");
        assert_eq!(result.skill.license.as_deref(), Some("MIT"));
        assert_eq!(result.skill.allowed_tools.as_deref(), Some("read_file"));
        assert_eq!(
            result.skill.metadata.get("version").map(String::as_str),
            Some("1")
        );
        assert_eq!(result.content, "Say hi.\n");
    }

    #[tokio::test]
    async fn an_unknown_skill_is_reported() {
        let root = scratch("unknown");

        let error = get_at(&root, "missing").await.unwrap_err();

        assert!(matches!(error, SkillsError::Unknown(name) if name == "missing"));
    }

    #[tokio::test]
    async fn names_that_are_not_one_component_are_rejected() {
        let root = scratch("names");

        for name in ["", ".", "..", "../secret", "a/b"] {
            let error = get_at(&root, name).await.unwrap_err();
            assert!(
                matches!(error, SkillsError::InvalidName(_)),
                "{name:?} was not rejected"
            );
        }
    }

    #[tokio::test]
    async fn an_invalid_skill_file_is_reported() {
        let root = scratch("invalid");
        fixture(&root, "greet", "description: no name here", "");

        let error = get_at(&root, "greet").await.unwrap_err();

        assert!(matches!(error, SkillsError::Invalid(name, _) if name == "greet"));
    }
}
