use std::collections::BTreeMap;
use std::path::PathBuf;

use agent_plugins::{SkillMeta, parse_skill_md};

use crate::events::{self, Digest};
use crate::path::{AGENTS_DIR, SKILL_MD, SKILLS_DIR};
use crate::plugin::{self, Plugins};

#[derive(Clone)]
pub struct Skill {
    /// The id the scope presents it under: the directory name, or
    /// `{plugin_id}.{directory}` for a skill shipped by a plugin.
    pub id: String,
    /// The skill directory, canonical.
    pub root: PathBuf,
    /// The `SKILL.md` it was discovered from, as content.
    pub digest: Digest,
    /// The validated `SKILL.md` frontmatter.
    pub meta: SkillMeta,
    /// The `SKILL.md` text after the frontmatter, as written.
    pub body: String,
}

#[derive(Clone)]
pub struct Skills {
    scope: String,
    root: PathBuf,
    plugins: Plugins,
}

impl Skills {
    pub fn new(scope: impl Into<String>, root: impl Into<PathBuf>, plugins: Plugins) -> Self {
        Self {
            scope: scope.into(),
            root: root.into(),
            plugins,
        }
    }

    pub fn scope(&self) -> &str {
        &self.scope
    }

    /// The scope's skills in id order, as the directories are right now.
    ///
    /// Discovery runs per request, so a directory that appears or disappears
    /// shows up in the next answer. A child directory the specification
    /// rejects is skipped, and a scope without a discovery directory holds
    /// nothing.
    pub async fn list(&self) -> Vec<Skill> {
        let mut skills: BTreeMap<String, Skill> = self
            .plugins
            .list()
            .iter()
            .flat_map(|plugin| &plugin.skills)
            .map(|skill| (skill.id.clone(), skill.clone()))
            .collect();

        let Ok(mut entries) = tokio::fs::read_dir(self.directory()).await else {
            return skills.into_values().collect();
        };

        while let Ok(Some(entry)) = entries.next_entry().await {
            let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if let Some(skill) = discover(entry.path(), &id).await {
                skills.insert(skill.id.clone(), skill);
            }
        }

        skills.into_values().collect()
    }

    /// The scope's skill `id`, as it is on disk right now.
    ///
    /// Resolved through [`Skills::list`], so a caller-supplied id reaches the
    /// skill directory of a skill this scope discovers and nothing else.
    pub async fn get(&self, id: &str) -> Option<Skill> {
        self.list().await.into_iter().find(|skill| skill.id == id)
    }

    /// The same scope, with the plugins it holds now.
    pub fn rescan(&self) -> Result<Self, plugin::Error> {
        Ok(Self {
            scope: self.scope.clone(),
            root: self.root.clone(),
            plugins: self.plugins.rescan()?,
        })
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
        id: id.to_owned(),
        root,
        digest: events::digest(&document),
        meta,
        body: body.source().to_owned(),
    })
}
