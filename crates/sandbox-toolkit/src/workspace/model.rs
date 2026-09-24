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
/// path inside the process.
///
/// [`WorkspaceRegistry::resolve`]: super::registry::WorkspaceRegistry::resolve
#[derive(Debug, Serialize, JsonSchema, PartialEq, Eq, TS)]
#[ts(export)]
pub(crate) struct Workspace {
    id: String,
}

impl Workspace {
    pub(crate) fn new(id: impl Into<String>) -> Self {
        Self { id: id.into() }
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
