use std::collections::HashMap;

use schemars::JsonSchema;
use serde::Serialize;
use ts_rs::TS;

/// The only transport a served MCP presents, whatever transport it was
/// declared with.
#[derive(Debug, Clone, Serialize, JsonSchema, TS)]
#[ts(export)]
pub struct StreamableHttpMcpServer {
    #[serde(rename = "type")]
    transport: String,
    pub url: String,
}

impl StreamableHttpMcpServer {
    pub fn new(url: String) -> Self {
        Self {
            transport: "streamable-http".to_owned(),
            url,
        }
    }
}

#[derive(Debug, Clone, Serialize, JsonSchema, TS)]
#[ts(export)]
pub struct StreamableHttpServerMcpConfig {
    #[serde(rename = "$schema")]
    schema: String,
    #[serde(rename = "mcpServers")]
    pub mcp_servers: HashMap<String, StreamableHttpMcpServer>,
}

impl StreamableHttpServerMcpConfig {
    pub fn new(mcp_servers: HashMap<String, StreamableHttpMcpServer>) -> Self {
        Self {
            schema: agent_plugins::MCP_SCHEMA_1_0_0.to_owned(),
            mcp_servers,
        }
    }
}

impl Default for StreamableHttpServerMcpConfig {
    fn default() -> Self {
        Self::new(HashMap::new())
    }
}
