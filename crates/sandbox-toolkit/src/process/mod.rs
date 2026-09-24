//! The `/process` resource API.
//!
//! exec and shell run over HTTP and share the length-prefixed frames in
//! [`exec`], while a pty session runs over a WebSocket and uses the per-message
//! frames in [`pty`]. exec is also exposed as an MCP tool, which answers with the
//! completed result because MCP has no frame stream to upgrade to. The wire types
//! exported to TypeScript clients live in [`model`] and are reused by the MCP tools
//! in [`mcp`].

mod exec;
mod http;
mod mcp;
mod model;
mod pty;

pub(crate) use self::http::router;
