//! The MCP resource-management tools over the HTTP control-plane models.

use rmcp::ErrorData;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::{tool, tool_router};

use super::model::{Mcp, McpId, McpList, RegisterMcpRequest};
use crate::server::{ToolkitServer, not_implemented};

#[tool_router(router = mcp_tools, vis = "pub(crate)")]
impl ToolkitServer {
    /// Register an MCP server and return its endpoint descriptor.
    #[tool(name = "register_mcp")]
    async fn register_mcp(
        &self,
        Parameters(_request): Parameters<RegisterMcpRequest>,
    ) -> Result<Json<Mcp>, ErrorData> {
        Err(not_implemented("register_mcp"))
    }

    /// List the registered MCP servers.
    #[tool(name = "list_mcps")]
    async fn list_mcps(&self) -> Result<Json<McpList>, ErrorData> {
        Err(not_implemented("list_mcps"))
    }

    /// Fetch a registered MCP server by id.
    #[tool(name = "get_mcp")]
    async fn get_mcp(&self, Parameters(_id): Parameters<McpId>) -> Result<Json<Mcp>, ErrorData> {
        Err(not_implemented("get_mcp"))
    }

    /// Delete a registered MCP server by id.
    #[tool(name = "delete_mcp")]
    async fn delete_mcp(&self, Parameters(_id): Parameters<McpId>) -> Result<(), ErrorData> {
        Err(not_implemented("delete_mcp"))
    }
}
