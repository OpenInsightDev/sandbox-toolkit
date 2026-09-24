//! Wire types of the Skill resource API.
//!
//! A skill's metadata is the top layer of the Agent Skills progressive
//! disclosure: the `SKILL.md` frontmatter fields together with the addressing
//! the service derives from discovery, without the body.

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::Serialize;
use ts_rs::TS;

/// Metadata of a discovered skill.
///
/// The frontmatter fields carry the skill's own declaration; `id`, `root`,
/// `uri` and `workspace_id` are the addressing discovery assigns it.
#[derive(Debug, Serialize, JsonSchema, TS)]
#[ts(export)]
#[allow(dead_code, reason = "built by the list handler, which is a stub")]
pub(crate) struct SkillMetadata {
    /// Directory name, which must equal the frontmatter `name`.
    id: String,
    /// Discovered skill directory, and the root of the derived read-only workspace.
    root: String,
    /// Skill name, equal to the directory name and the `id`.
    name: String,
    /// One-line description of what the skill does and when to use it.
    description: String,
    /// License the skill is distributed under.
    #[serde(skip_serializing_if = "Option::is_none")]
    license: Option<String>,
    /// Environment the skill needs, such as required tools.
    #[serde(skip_serializing_if = "Option::is_none")]
    compatibility: Option<String>,
    /// Free-form frontmatter fields reserved for clients.
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<HashMap<String, String>>,
    /// Address of the skill's body at this mount point.
    uri: String,
    /// Id of the read-only workspace holding the skill's other files.
    workspace_id: String,
}

/// The skills discovered under one mount point, in discovery order.
#[derive(Debug, Serialize, JsonSchema, TS)]
#[ts(export)]
#[allow(dead_code, reason = "built by the list handler, which is a stub")]
pub(crate) struct SkillList {
    skills: Vec<SkillMetadata>,
}
