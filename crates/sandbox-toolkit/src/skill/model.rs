use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Serialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct SkillMetadata {
    /// Directory name, which must equal the frontmatter `name`.
    pub(crate) id: String,
    /// The derived read-only workspace root.
    pub(crate) root: String,
    pub(crate) name: String,
    pub(crate) description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub(crate) license: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub(crate) compatibility: Option<String>,
    /// Free-form frontmatter fields reserved for clients.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub(crate) metadata: Option<HashMap<String, String>>,
    /// Address of the skill's body at this mount point.
    pub(crate) uri: String,
    /// The read-only workspace holding the skill's other files.
    pub(crate) workspace_id: String,
}

#[derive(Debug, Serialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct SkillList {
    pub(crate) skills: Vec<SkillMetadata>,
}

/// Addressing is a model field, so one request serves both mounts: HTTP folds
/// `workspace_id` in from the path, MCP takes it directly.
#[derive(Debug, Default, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct ListRequest {
    /// Absent selects the global mount.
    #[serde(default)]
    pub(crate) workspace_id: Option<String>,
    /// Absent starts at the first skill.
    #[serde(default)]
    pub(crate) offset: Option<u64>,
    /// Absent returns every remaining skill.
    #[serde(default)]
    pub(crate) limit: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct GetRequest {
    /// Absent selects the global mount.
    #[serde(default)]
    pub(crate) workspace_id: Option<String>,
    pub(crate) skill_id: String,
}

#[derive(Debug, Serialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct SkillBody {
    pub(crate) skill_id: String,
    pub(crate) content: String,
}
