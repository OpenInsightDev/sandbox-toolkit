use rmcp::ErrorData;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::{tool, tool_router};

use super::model::{CreateWorkspaceRequest, Workspace, WorkspaceId, WorkspaceList};
use crate::server::{ToolkitServer, not_implemented};

#[tool_router(router = workspace_tools, vis = "pub(crate)")]
impl ToolkitServer {
    /// Register a remote absolute path as a workspace and return its handle.
    #[tool(name = "create_workspace")]
    async fn create_workspace(
        &self,
        Parameters(_request): Parameters<CreateWorkspaceRequest>,
    ) -> Result<Json<Workspace>, ErrorData> {
        Err(not_implemented("create_workspace"))
    }

    /// List the registered workspaces.
    #[tool(name = "list_workspaces")]
    async fn list_workspaces(&self) -> Result<Json<WorkspaceList>, ErrorData> {
        Err(not_implemented("list_workspaces"))
    }

    /// Fetch a workspace by id.
    #[tool(name = "get_workspace")]
    async fn get_workspace(
        &self,
        Parameters(_id): Parameters<WorkspaceId>,
    ) -> Result<Json<Workspace>, ErrorData> {
        Err(not_implemented("get_workspace"))
    }

    /// Delete a workspace by id.
    #[tool(name = "delete_workspace")]
    async fn delete_workspace(
        &self,
        Parameters(_id): Parameters<WorkspaceId>,
    ) -> Result<(), ErrorData> {
        Err(not_implemented("delete_workspace"))
    }
}
