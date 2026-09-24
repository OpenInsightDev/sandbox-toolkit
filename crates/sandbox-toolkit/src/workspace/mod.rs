//! The workspace control plane.
//!
//! A workspace registers a remote absolute path under a caller-chosen id and is
//! the boundary every path-bearing operation is confined to; other resource
//! APIs reuse it, such as an MCP stdio entry that runs inside one. The wire
//! types live in [`model`] and are reused by the MCP tools in [`mcp`].
//!
//! The registry itself is [`WorkspaceRegistry`], owned by [`crate::AppState`];
//! [`http`] and [`mcp`] are its call surfaces, and [`WorkspaceRegistry::resolve`]
//! is the resolution API the other modules build on.

mod http;
mod mcp;
pub(crate) mod model;
pub(crate) mod registry;

pub(crate) use self::http::router;
pub(crate) use self::registry::WorkspaceRegistry;
