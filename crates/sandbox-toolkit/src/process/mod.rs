//! exec carries both the executable and shell payloads over the length-prefixed
//! frames in [`frame`], while a pty session runs over a WebSocket and uses the
//! per-message frames in [`pty`]. exec is also an MCP tool, which answers with the
//! completed result because MCP has no frame stream to upgrade to.

mod exec;
mod frame;
mod http;
mod mcp;
mod model;
mod pty;

pub(crate) use self::http::router;
