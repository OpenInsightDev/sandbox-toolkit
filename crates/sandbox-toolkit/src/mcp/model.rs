use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// A closed union discriminated by `type`: the fields belong to whichever
/// transport the tag names.
#[derive(Debug, Deserialize, Serialize, JsonSchema, TS)]
#[ts(export)]
#[serde(rename_all = "kebab-case", tag = "type")]
pub(crate) enum McpServer {
    /// A local child process the server starts and bridges over Streamable HTTP.
    Stdio {
        /// A bare name resolved through `PATH`, or a path starting with `./`
        /// resolved against `cwd`.
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: HashMap<String, String>,
        /// Workspace-relative in workspace mode, a normalized absolute path in
        /// direct mode.
        cwd: Option<String>,
    },
    /// A remote server reached through the server's reverse proxy.
    StreamableHttp {
        url: String,
        /// Literal, visible configuration headers; never sensitive data.
        #[serde(default)]
        headers: HashMap<String, String>,
    },
}

#[derive(Debug, Deserialize, JsonSchema, TS)]
#[ts(export)]
#[expect(
    dead_code,
    reason = "read by the registration handler, which is a stub"
)]
pub(crate) struct RegisterMcpRequest {
    pub(crate) id: String,
    pub(crate) server: McpServer,
}

#[derive(Debug, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct McpId {
    #[expect(dead_code, reason = "read by the MCP tool wrapper")]
    pub(crate) mcp_id: String,
}

/// A handle to a registered server's endpoint.
#[derive(Debug, Serialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct Mcp {
    id: String,
    server: McpServer,
    /// Server-relative path of the Streamable HTTP endpoint that serves the entry.
    uri: String,
}

#[derive(Debug, Serialize, JsonSchema, TS)]
#[ts(export)]
// `expect` cannot be used here: the ts-rs export test makes the type live under
// `cfg(test)`, while it is otherwise unreferenced.
#[allow(dead_code, reason = "built by the list handler, which is a stub")]
pub(crate) struct McpList {
    mcps: Vec<Mcp>,
}

/// An Agent Plugins `mcp.json` document exported by the collection routes.
#[derive(Debug, Serialize, JsonSchema, TS)]
#[ts(export)]
#[allow(dead_code, reason = "built by the export handlers, which are stubs")]
pub(crate) struct McpDocument {
    #[serde(rename = "$schema")]
    schema: String,
    #[serde(rename = "mcpServers")]
    mcp_servers: HashMap<String, McpDocumentServer>,
}

/// Always presented as Streamable HTTP.
#[derive(Debug, Serialize, JsonSchema, TS)]
#[ts(export)]
#[allow(dead_code, reason = "built by the export handlers, which are stubs")]
pub(crate) struct McpDocumentServer {
    #[serde(rename = "type")]
    transport: String,
    url: String,
}
