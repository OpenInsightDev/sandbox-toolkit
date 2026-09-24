//! The MCP resource API.
//!
//! Registration and management live in [`http`] and [`mcp`]; the wire types live in [`model`].

mod http;
mod mcp;
pub(crate) mod model;

pub(crate) use self::http::router;
