//! The process MCP tools, over the exec model and operation.

use rmcp::ErrorData;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::{tool, tool_router};

use super::model::{ExecResult, ExecToolRequest};
use crate::server::{ToolkitServer, not_implemented};

#[tool_router(router = process_tools, vis = "pub(crate)")]
impl ToolkitServer {
    /// Run an executable with verbatim arguments, without a shell.
    ///
    /// The MCP counterpart of `POST .../exec`: it always returns the command's
    /// complete result, since MCP is a single request-response transport and has no
    /// frame stream to upgrade to.
    #[tool(name = "exec")]
    async fn exec(
        &self,
        Parameters(_request): Parameters<ExecToolRequest>,
    ) -> Result<Json<ExecResult>, ErrorData> {
        Err(not_implemented("exec"))
    }
}
