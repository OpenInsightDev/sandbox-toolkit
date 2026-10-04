use std::collections::BTreeMap;
use std::path::PathBuf;

use agent_plugins::{SkillMeta, parse_skill_md};

use crate::path::{AGENTS_DIR, SKILL_MD, SKILLS_DIR};

pub struct Skill {
    /// The skill directory, canonical.
    pub root: PathBuf,
    /// The validated `SKILL.md` frontmatter.
    pub meta: SkillMeta,
    /// The `SKILL.md` text after the frontmatter, as written.
    pub body: String,
}

impl Skill {
    /// The skill id: the directory name, which the specification requires
    /// `name` to match.
    pub fn id(&self) -> &str {
        &self.meta.name
    }
}

#[derive(Clone)]
pub struct Skills {
    scope: String,
    root: PathBuf,
}

impl Skills {
    pub fn new(scope: impl Into<String>, root: impl Into<PathBuf>) -> Self {
        Self {
            scope: scope.into(),
            root: root.into(),
        }
    }

    pub fn scope(&self) -> &str {
        &self.scope
    }

    /// The scope's skills in id order, as the directory is right now.
    ///
    /// Discovery runs per request, so a directory that appears or disappears
    /// shows up in the next answer. A child directory the specification
    /// rejects is skipped, and a scope without a discovery directory holds
    /// nothing.
    pub async fn list(&self) -> Vec<Skill> {
        let Ok(mut entries) = tokio::fs::read_dir(self.directory()).await else {
            return Vec::new();
        };

        let mut skills = BTreeMap::new();
        while let Ok(Some(entry)) = entries.next_entry().await {
            let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if let Some(skill) = discover(entry.path(), &id).await {
                skills.insert(skill.id().to_owned(), skill);
            }
        }

        skills.into_values().collect()
    }

    /// The scope's skill `id`, as it is on disk right now.
    ///
    /// Resolved through [`Skills::list`], so a caller-supplied id reaches the
    /// skill directory of a skill this scope discovers and nothing else.
    pub async fn get(&self, id: &str) -> Option<Skill> {
        self.list().await.into_iter().find(|skill| skill.id() == id)
    }

    fn directory(&self) -> PathBuf {
        self.root.join(AGENTS_DIR).join(SKILLS_DIR)
    }
}

/// The skill `id` in `directory`, `None` when the specification rejects it.
async fn discover(directory: PathBuf, id: &str) -> Option<Skill> {
    let document = tokio::fs::read(directory.join(SKILL_MD)).await.ok()?;
    let (meta, body) = parse_skill_md(id, &document).ok()?;
    let root = tokio::fs::canonicalize(&directory).await.ok()?;

    Some(Skill {
        root,
        meta,
        body: body.source().to_owned(),
    })
}
