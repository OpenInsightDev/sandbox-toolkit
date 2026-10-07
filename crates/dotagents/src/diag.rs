//! Diagnostics: the specification's "SHOULD report" made concrete.
//!
//! Agent Plugins is deliberately forgiving — most failures narrow to the
//! smallest boundary and loading continues. What must not happen is
//! *silence*. Every non-fatal decision the loader takes on the package's
//! behalf is recorded as a [`Diagnostic`]: which [`Rule`] fired, where
//! ([`Origin`]), and a human-readable why.

use std::fmt;

/// The closed set of non-fatal report conditions the specification defines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Rule {
    /// An unknown top-level manifest field was reported and ignored.
    UnknownManifestField,
    /// A non-object `extensions` field was reported and ignored.
    ExtensionsIgnored,
    /// A fixed component location exists but is unusable.
    ComponentInvalid,
    /// A discovered skill was skipped.
    SkillSkipped,
    /// An individual MCP server entry was skipped.
    ServerSkipped,
    /// MCP was disabled for the whole package.
    McpDisabled,
}

impl Rule {
    /// Stable kebab-case identifier, shared with the reference implementation.
    pub fn code(self) -> &'static str {
        match self {
            Self::UnknownManifestField => "unknown-manifest-field",
            Self::ExtensionsIgnored => "extensions-ignored",
            Self::ComponentInvalid => "component-invalid",
            Self::SkillSkipped => "skill-skipped",
            Self::ServerSkipped => "server-skipped",
            Self::McpDisabled => "mcp-disabled",
        }
    }

    /// The specification section that defines this report condition.
    pub fn section(self) -> &'static str {
        match self {
            Self::UnknownManifestField => "§5.2",
            Self::ExtensionsIgnored => "§8.1",
            Self::ComponentInvalid => "§6.2",
            Self::SkillSkipped => "§7.1",
            Self::ServerSkipped | Self::McpDisabled => "§7.2.2",
        }
    }
}

impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.code(), self.section())
    }
}

/// Where in the package a diagnostic points.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Origin {
    /// The `plugin.json` manifest.
    Manifest,
    /// The `skills/` component location.
    Skills,
    /// One skill directory under `skills/`, by directory name.
    Skill(String),
    /// The `mcp.json` component location.
    Mcp,
    /// One MCP server entry, by its `mcpServers` member name.
    Server(String),
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Manifest => f.write_str("plugin.json"),
            Self::Skills => f.write_str("skills/"),
            Self::Skill(dir) => write!(f, "skills/{dir}/"),
            Self::Mcp => f.write_str("mcp.json"),
            Self::Server(name) => write!(f, "mcp.json#{name}"),
        }
    }
}

/// One reported, non-fatal loading decision.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Diagnostic {
    /// Which report condition fired.
    pub rule: Rule,
    /// Where in the package it fired.
    pub origin: Origin,
    /// Human-readable explanation.
    pub message: String,
}

impl Diagnostic {
    pub(crate) fn new(rule: Rule, origin: Origin, message: impl Into<String>) -> Self {
        Self { rule, origin, message: message.into() }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at {}: {}", self.rule, self.origin, self.message)
    }
}
