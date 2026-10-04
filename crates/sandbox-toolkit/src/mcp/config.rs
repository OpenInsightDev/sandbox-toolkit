use crate::path::{AGENTS_DIR, MCP_JSON};
use agent_plugins::{
    Anchors, McpConfig, McpDisabledReason, McpServer as PluginMcpServer, ServerEntry, SpecVersion,
};
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum UnsupportedReason {
    #[error("deprecated")]
    Deprecated,
    #[error("unknown")]
    Unknown,
}

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Read(#[from] std::io::Error),
    #[error(transparent)]
    Disabled(#[from] McpDisabledReason),
    #[error("mcp server `{name}` uses unsupported transport `{transport}`: {reason}")]
    Unsupported {
        name: String,
        transport: String,
        reason: UnsupportedReason,
    },
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub id: String,
    pub server: PluginMcpServer,
    pub anchors: Anchors,
}

pub async fn entries(root: &Path) -> Result<Vec<Entry>, Error> {
    let agents_dir = root.join(AGENTS_DIR);
    let bytes = match tokio::fs::read(agents_dir.join(MCP_JSON)).await {
        Ok(bytes) => bytes,
        // A scope that declares no servers is not a failure: it simply has none.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };

    let (config, _) = McpConfig::parse(&bytes, SpecVersion::V1_0_0)?;

    let mut entries = Vec::new();
    for ServerEntry { server, name, .. } in config.servers {
        let server = match &server {
            PluginMcpServer::Stdio(_) | PluginMcpServer::StreamableHttp(_) => Ok(server),
            PluginMcpServer::Sse(_) => Err(Error::Unsupported {
                name: name.to_owned(),
                transport: "sse".to_owned(),
                reason: UnsupportedReason::Deprecated,
            }),
            other => Err(Error::Unsupported {
                name: name.to_owned(),
                transport: other.transport().to_string(),
                reason: UnsupportedReason::Unknown,
            }),
        }?;
        let anchors = Anchors::new(&agents_dir, &agents_dir.join(".data").join(&name));
        entries.push(Entry {
            id: name,
            server,
            anchors,
        });
    }

    Ok(entries)
}
