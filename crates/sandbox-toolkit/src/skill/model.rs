use std::collections::HashMap;

use schemars::JsonSchema;
use serde::Serialize;
use ts_rs::TS;

/// The top layer of the Agent Skills progressive disclosure: the `SKILL.md`
/// frontmatter fields plus the addressing discovery assigns, without the body.
#[derive(Debug, Serialize, JsonSchema, TS)]
#[ts(export)]
#[allow(dead_code, reason = "built by the list handler, which is a stub")]
pub(crate) struct SkillMetadata {
    /// Directory name, which must equal the frontmatter `name`.
    id: String,
    /// The root of the derived read-only workspace.
    root: String,
    name: String,
    description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    license: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    compatibility: Option<String>,
    /// Free-form frontmatter fields reserved for clients.
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<HashMap<String, String>>,
    /// Address of the skill's body at this mount point.
    uri: String,
    /// The read-only workspace holding the skill's other files.
    workspace_id: String,
}

#[derive(Debug, Serialize, JsonSchema, TS)]
#[ts(export)]
#[allow(dead_code, reason = "built by the list handler, which is a stub")]
pub(crate) struct SkillList {
    skills: Vec<SkillMetadata>,
}
