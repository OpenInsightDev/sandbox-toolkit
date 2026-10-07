//! The business-semantic events a [`Watcher`](super::Watcher) emits.
//!
//! Each `change` variant carries exactly the content its transition has: the
//! parsed resource when it is valid, the reason when it is not, and nothing
//! when it is gone.

use crate::diag::Diagnostic;
use crate::mcp::ServerEntry;
use crate::name::PluginName;
use crate::plugin::{Manifest, ManifestRejection};
use crate::skill::Skill;

/// One business-semantic change inside a watched package.
///
/// `plugin` is the manifest name known *after* the change; it is `None` while
/// the package carries no readable manifest.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Event {
    /// The `plugin.json` manifest.
    Plugin {
        plugin: Option<PluginName>,
        change: PluginChange,
    },
    /// One skill under `skills/`, by its directory name.
    Skill {
        plugin: Option<PluginName>,
        id: String,
        change: SkillChange,
    },
    /// The `mcp.json` server set.
    Mcp {
        plugin: Option<PluginName>,
        change: McpChange,
    },
}

/// How the manifest changed.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PluginChange {
    /// A manifest appeared, or a package of this name replaced another.
    Added(Manifest),
    /// The manifest changed without changing the plugin name.
    Modified(Manifest),
    /// A manifest is present but fatally invalid, so the package is rejected.
    /// The flags record whether the previous state still held those
    /// components, which the consumer must now retire.
    Invalid {
        /// The fatal manifest problem.
        rejection: ManifestRejection,
        /// Whether skills were still held.
        skills: bool,
        /// Whether the MCP component was still held.
        mcp: bool,
    },
    /// No usable manifest remains. The flags are as for [`Self::Invalid`].
    Removed {
        /// Whether skills were still held.
        skills: bool,
        /// Whether the MCP component was still held.
        mcp: bool,
    },
}

/// How one skill changed.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SkillChange {
    /// The skill entered the valid set.
    Added(Skill),
    /// A valid skill's content changed.
    Modified(Skill),
    /// `SKILL.md` is present but no longer validates.
    Invalid(Vec<Diagnostic>),
    /// The skill disappeared.
    Removed,
}

/// How the `mcp.json` server set changed.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum McpChange {
    /// `mcp.json` was reloaded and the server set differs; at least one side is
    /// non-empty. A server whose configuration changed appears on both sides.
    Reloaded {
        /// Servers that were not present before.
        added: Vec<ServerEntry>,
        /// Servers that are no longer present.
        removed: Vec<ServerEntry>,
    },
    /// `mcp.json` is present but does not validate; MCP is disabled for the
    /// package, so every previously known server is gone.
    Invalid(Vec<Diagnostic>),
}

impl Event {
    /// The manifest name this event was derived under, if known.
    pub fn plugin(&self) -> Option<&PluginName> {
        match self {
            Self::Plugin { plugin, .. }
            | Self::Skill { plugin, .. }
            | Self::Mcp { plugin, .. } => plugin.as_ref(),
        }
    }
}
