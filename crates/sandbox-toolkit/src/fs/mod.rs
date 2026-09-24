//! The file API of a workspace.
//!
//! Files map directly to URIs under the workspace root, with the routes
//! built in [`files`]. The wire types shared by the control and data plane live
//! in [`model`] and are reused by the MCP tools defined in this module.

mod dir;
mod files;
pub(crate) mod model;
mod read;

use axum::Router;
use rmcp::ErrorData;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::{tool, tool_router};

use self::model::{
    DirectoryResponse, FileMetadata, FilePath, RelocateFileRequest, WriteFileRequest,
};
use crate::AppState;
use crate::mcp::{ToolkitServer, not_implemented};

/// Build the file router.
///
/// Paths include the owning workspace because a file is always addressed
/// within one, but the routes stand on their own rather than being nested into
/// the workspace control plane.
pub(crate) fn router() -> Router<AppState> {
    // A wildcard capture never matches an empty path, and `/fs` and `/fs/`
    // are distinct routes in axum, so addressing the workspace root needs these
    // two routes; the wildcard route matches neither of them.
    Router::new()
        .route("/workspaces/{workspace_id}/fs", files::file_routes())
        .route("/workspaces/{workspace_id}/fs/", files::file_routes())
        .route(
            "/workspaces/{workspace_id}/fs/{*path}",
            files::file_routes(),
        )
        .route("/fs", files::file_routes())
        .route("/fs/", files::file_routes())
        .route("/fs/{*path}", files::file_routes())
}

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
