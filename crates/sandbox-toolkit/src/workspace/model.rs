//! Wire types of the workspace control plane.
//!
//! The same types describe the workspace MCP tools, so their `JsonSchema`
//! derivations are the tool input and output schemas.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// `root` is a remote absolute path, accepted only at this control-plane boundary.
#[derive(Debug, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct CreateWorkspaceRequest {
    /// Unique workspace identifier chosen by the caller.
    pub(crate) id: String,
    /// Remote absolute path registered as the workspace root.
    pub(crate) root: String,
    /// Constraints narrowing operations inside the workspace; omitted properties
    /// take their defaults.
    #[serde(default)]
    pub(crate) properties: WorkspaceProperties,
}

/// File read/write mode of a workspace.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(export)]
pub(crate) enum WorkspaceAccess {
    /// Reads and writes are both allowed.
    #[default]
    ReadWrite,
    /// Mutation operations are rejected; reads are unaffected.
    ReadOnly,
}

/// Constraints fixed at registration that narrow every operation inside a
/// workspace.
///
/// A missing property takes its default, so a workspace always carries a
/// complete set of effective values.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct WorkspaceProperties {
    /// Read/write mode of the workspace's files.
    #[serde(default)]
    pub(crate) access: WorkspaceAccess,
}

/// Selects an existing workspace by id.
#[derive(Debug, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct WorkspaceId {
    /// Identifier of the target workspace.
    pub(crate) workspace_id: String,
}

/// A registered workspace, exposed to clients as an opaque handle.
///
/// The root is deliberately absent: once registered under an id it is reachable
/// only through [`WorkspaceRegistry::resolve`], which keeps the remote absolute
/// path inside the process. The properties are echoed back resolved, so a client
/// reads the effective constraints rather than the ones it requested.
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
