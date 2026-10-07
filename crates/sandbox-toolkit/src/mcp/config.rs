use crate::path::{AGENTS_DIR, DATA_DIR, MCP_JSON};
use crate::plugin::Plugins;
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
    /// Whether the launched child carries the reserved variables. Only a
    /// plugin's entry does, so a scope's own `mcp.json` cannot leak them.
    pub reserved: bool,
}

/// The servers a scope serves: its own `mcp.json`, then its plugins'.
pub async fn entries(root: &Path, plugins: &Plugins) -> Result<Vec<Entry>, Error> {
    let mut entries = scope_entries(root).await?;
    for plugin in plugins.list() {
        let anchors = Anchors::new(&plugin.root, plugin.root.join(DATA_DIR));
        for ServerEntry { name, server, .. } in &plugin.servers {
            // A plugin entry the specification permits but this service cannot
            // run is skipped like any other rejected entry, not fatal.
            if supported(server).is_err() {
                continue;
            }
            entries.push(Entry {
                id: format!("{}.{name}", plugin.id),
                server: server.clone(),
                anchors: anchors.clone(),
                reserved: true,
            });
        }
    }

    Ok(entries)
}

async fn scope_entries(root: &Path) -> Result<Vec<Entry>, Error> {
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
        if let Err(reason) = supported(&server) {
            return Err(Error::Unsupported {
                name: name.to_owned(),
                transport: server.transport().to_string(),
                reason,
            });
        }
        let anchors = Anchors::new(&agents_dir, agents_dir.join(DATA_DIR).join(&name));
        entries.push(Entry {
            id: name,
            server,
            anchors,
            reserved: false,
        });
    }

    Ok(entries)
}

fn supported(server: &PluginMcpServer) -> Result<(), UnsupportedReason> {
    match server {
        PluginMcpServer::Stdio(_) | PluginMcpServer::StreamableHttp(_) => Ok(()),
        PluginMcpServer::Sse(_) => Err(UnsupportedReason::Deprecated),
        _ => Err(UnsupportedReason::Unknown),
    }
}
