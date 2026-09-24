//! The resource MCP tools, over the resource API's model.

use rmcp::ErrorData;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::{tool, tool_router};

use super::model::{
    DirectoryResponse, RelocateResourceRequest, ResourceMetadata, ResourcePath, WriteFileRequest,
};
use crate::server::{ToolkitServer, not_implemented};

#[tool_router(router = resource_tools, vis = "pub(crate)")]
impl ToolkitServer {
    /// Read metadata for a file, directory or symlink.
    #[tool(name = "stat_resource")]
    async fn stat_resource(
        &self,
        Parameters(_resource): Parameters<ResourcePath>,
    ) -> Result<Json<ResourceMetadata>, ErrorData> {
        Err(not_implemented("stat_resource"))
    }

    /// List a directory and its direct children.
    #[tool(name = "list_directory")]
    async fn list_directory(
        &self,
        Parameters(_resource): Parameters<ResourcePath>,
    ) -> Result<Json<DirectoryResponse>, ErrorData> {
        Err(not_implemented("list_directory"))
    }

    /// Write a file, replacing any existing content.
    #[tool(name = "write_file")]
    async fn write_file(
        &self,
        Parameters(_request): Parameters<WriteFileRequest>,
    ) -> Result<Json<ResourceMetadata>, ErrorData> {
        Err(not_implemented("write_file"))
    }

    /// Delete a file or directory.
    #[tool(name = "delete_resource")]
    async fn delete_resource(
        &self,
        Parameters(_resource): Parameters<ResourcePath>,
    ) -> Result<(), ErrorData> {
        Err(not_implemented("delete_resource"))
    }

    /// Copy or move a file or directory within a workspace.
    #[tool(name = "copy_or_move_resource")]
    async fn copy_or_move_resource(
        &self,
        Parameters(_request): Parameters<RelocateResourceRequest>,
    ) -> Result<Json<ResourceMetadata>, ErrorData> {
        Err(not_implemented("copy_or_move_resource"))
    }
}
