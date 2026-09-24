//! Wire types of the MCP resource API.

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The transport an MCP server is declared with, a closed union discriminated by
/// `type`: the fields belong to whichever transport the tag names.
#[derive(Debug, Deserialize, Serialize, JsonSchema, TS)]
#[ts(export)]
#[serde(rename_all = "kebab-case", tag = "type")]
pub(crate) enum McpServer {
    /// A local child process the server starts and bridges over Streamable HTTP.
    Stdio {
        /// Executable to run: a bare name resolved through `PATH`, or a path starting
        /// with `./` resolved against `cwd`.
        command: String,
        /// Arguments passed verbatim, one argv entry each.
        #[serde(default)]
        args: Vec<String>,
        /// Variables layered over the inherited environment.
        #[serde(default)]
        env: HashMap<String, String>,
        /// Working directory: workspace-relative in workspace mode, a normalized
        /// absolute path in direct mode.
        cwd: Option<String>,
    },
    /// A remote server reached through the server's reverse proxy.
    StreamableHttp {
        /// Absolute `http`/`https` URL of the remote server.
        url: String,
        /// Literal, visible configuration headers; never sensitive data.
        #[serde(default)]
        headers: HashMap<String, String>,
    },
}

/// Parameters of the registration routes, `POST .../mcps`.
#[derive(Debug, Deserialize, JsonSchema, TS)]
#[ts(export)]
#[expect(
    dead_code,
    reason = "read by the registration handler, which is a stub"
)]
pub(crate) struct RegisterMcpRequest {
    /// Unique MCP identifier chosen by the caller.
    pub(crate) id: String,
    /// Server configuration, discriminated by its transport.
    pub(crate) server: McpServer,
}

/// Selects a registered MCP server by id.
#[derive(Debug, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct McpId {
    /// Identifier of the MCP server.
    #[expect(dead_code, reason = "read by the MCP tool wrapper")]
    pub(crate) mcp_id: String,
}

/// A registered MCP server, exposed to clients as a handle to its endpoint.
#[derive(Debug, Serialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct Mcp {
    /// Identifier the server is registered under.
    id: String,
    /// Server configuration as registered.
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
    /// Fixed schema identifier of the document.
    #[serde(rename = "$schema")]
    schema: String,
    /// Every available MCP, whether registered as stdio or Streamable HTTP.
    #[serde(rename = "mcpServers")]
    mcp_servers: HashMap<String, McpDocumentServer>,
}

/// One entry of [`McpDocument`], always presented as Streamable HTTP.
#[derive(Debug, Serialize, JsonSchema, TS)]
#[ts(export)]
#[allow(dead_code, reason = "built by the export handlers, which are stubs")]
pub(crate) struct McpDocumentServer {
    /// Transport tag of the entry.
    #[serde(rename = "type")]
    transport: String,
    /// Absolute URL of the entry's endpoint.
    url: String,
}
