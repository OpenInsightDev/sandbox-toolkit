use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// `root` is a remote absolute path, accepted only at this control-plane boundary.
#[derive(Debug, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct CreateWorkspaceRequest {
    pub(crate) id: String,
    pub(crate) root: String,
    /// Omitted properties take their defaults.
    #[serde(default)]
    pub(crate) properties: WorkspaceProperties,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(export)]
pub(crate) enum WorkspaceAccess {
    #[default]
    ReadWrite,
    /// Mutations are rejected; reads are unaffected.
    ReadOnly,
}

/// Fixed at registration, so a workspace always carries a complete set of
/// effective values.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct WorkspaceProperties {
    #[serde(default)]
    pub(crate) access: WorkspaceAccess,
}

#[derive(Debug, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct WorkspaceId {
    pub(crate) workspace_id: String,
}

/// The root is deliberately absent: once registered under an id it is reachable
/// only through [`WorkspaceRegistry::resolve`], keeping the remote absolute path
/// inside the process. Properties are echoed back resolved, so a client reads the
/// effective constraints rather than the ones it requested.
///
/// [`WorkspaceRegistry::resolve`]: super::registry::WorkspaceRegistry::resolve
#[derive(Debug, Serialize, JsonSchema, PartialEq, Eq, TS)]
#[ts(export)]
pub(crate) struct Workspace {
    id: String,
    properties: WorkspaceProperties,
}

impl Workspace {
    pub(crate) fn new(id: impl Into<String>, properties: WorkspaceProperties) -> Self {
        Self {
            id: id.into(),
            properties,
        }
    }

    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "read by tests and, later, callers that need the handle's id"
        )
    )]
    pub(crate) fn id(&self) -> &str {
        &self.id
    }
}

#[derive(Debug, Serialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct WorkspaceList {
    workspaces: Vec<Workspace>,
}

impl WorkspaceList {
    pub(crate) fn new(workspaces: Vec<Workspace>) -> Self {
        Self { workspaces }
    }
}
