//! The file MCP tools, over the file API's model.

use rmcp::ErrorData;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::{tool, tool_router};

use super::model::{
    DirectoryResponse, FileMetadata, FilePath, RelocateFileRequest, WriteFileRequest,
};
use crate::server::{ToolkitServer, not_implemented};

#[tool_router(router = file_tools, vis = "pub(crate)")]
impl ToolkitServer {
    /// Read metadata for a file, directory or symlink.
    #[tool(name = "stat_file")]
    async fn stat_file(
        &self,
        Parameters(_file): Parameters<FilePath>,
    ) -> Result<Json<FileMetadata>, ErrorData> {
        Err(not_implemented("stat_file"))
    }

    /// List a directory and its direct children.
    #[tool(name = "list_directory")]
    async fn list_directory(
        &self,
        Parameters(_file): Parameters<FilePath>,
    ) -> Result<Json<DirectoryResponse>, ErrorData> {
        Err(not_implemented("list_directory"))
    }

    /// Write a file, replacing any existing content.
    #[tool(name = "write_file")]
    async fn write_file(
        &self,
        Parameters(_request): Parameters<WriteFileRequest>,
    ) -> Result<Json<FileMetadata>, ErrorData> {
        Err(not_implemented("write_file"))
    }

    /// Delete a file or directory.
    #[tool(name = "delete_file")]
    async fn delete_file(&self, Parameters(_file): Parameters<FilePath>) -> Result<(), ErrorData> {
        Err(not_implemented("delete_file"))
    }

    /// Copy or move a file or directory within a workspace.
    #[tool(name = "copy_or_move_file")]
    async fn copy_or_move_file(
        &self,
        Parameters(_request): Parameters<RelocateFileRequest>,
    ) -> Result<Json<FileMetadata>, ErrorData> {
        Err(not_implemented("copy_or_move_file"))
    }
}
