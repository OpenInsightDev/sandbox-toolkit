use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(export)]
pub enum WorkspaceAccess {
    #[default]
    ReadWrite,
    ReadOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub struct Metadata {
    pub id: String,
    pub root: PathBuf,
    #[serde(default)]
    pub access: WorkspaceAccess,
}

/// `root` is a remote absolute path, accepted only at this control-plane boundary.
#[derive(Debug, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub struct CreateWorkspaceRequest {
    pub id: String,
    pub root: String,
    #[serde(default)]
    pub access: WorkspaceAccess,
}

#[derive(Debug, Serialize, JsonSchema, TS)]
#[ts(export)]
pub struct WorkspaceList(pub Vec<Metadata>);

impl WorkspaceList {
    pub fn new(metadata: Vec<Metadata>) -> Self {
        Self(metadata)
    }
}
