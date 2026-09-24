//! The file API of a workspace.
//!
//! Files map directly to URIs under the workspace root, with the routes built
//! in [`http`]. The wire types shared by the control and data plane live in
//! [`model`] and are reused by the MCP tools in [`mcp`].

mod http;
mod mcp;
pub(crate) mod model;

mod dir;
mod file;

pub(crate) use self::http::router;
