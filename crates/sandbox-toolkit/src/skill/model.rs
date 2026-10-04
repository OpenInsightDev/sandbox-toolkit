use std::collections::BTreeMap;
use std::path::PathBuf;

use schemars::JsonSchema;
use serde::Serialize;
use ts_rs::TS;

use super::discover::Skill;
use super::id::derived_workspace_id;

#[derive(Debug, Clone, Serialize, JsonSchema, TS)]
#[ts(export)]
pub struct SkillMetadata {
    pub id: String,
    pub root: PathBuf,
    pub name: String,
    pub description: String,
    /// The optional frontmatter fields; one the frontmatter did not write stays
    /// absent from the answer.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub license: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub compatibility: Option<String>,
    /// The frontmatter's mapping, `None` when it wrote none — which an empty
    /// mapping also means.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub metadata: Option<BTreeMap<String, String>>,
    /// Where the mount that answered serves this skill's body.
    pub uri: String,
    /// The workspace the discovering scope derives for this skill.
    pub workspace_id: String,
}

impl SkillMetadata {
    pub fn new(scope: &str, skill: &Skill, base: &str) -> Self {
        let id = &skill.id;
        Self {
            id: id.to_owned(),
            root: skill.root.clone(),
            name: skill.meta.name.clone(),
            description: skill.meta.description.clone(),
            license: skill.meta.license.clone(),
            compatibility: skill.meta.compatibility.clone(),
            metadata: (!skill.meta.metadata.is_empty()).then(|| skill.meta.metadata.clone()),
            uri: format!("{base}/{id}"),
            workspace_id: derived_workspace_id(scope, id),
        }
    }
}

#[derive(Debug, Clone, Serialize, JsonSchema, TS)]
#[ts(export)]
pub struct SkillList {
    pub skills: Vec<SkillMetadata>,
}
