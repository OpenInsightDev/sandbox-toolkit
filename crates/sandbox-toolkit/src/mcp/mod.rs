//! The MCP resource API.
//!
//! Registration and management live in [`http`]; the wire types live in [`model`].

mod http;
pub(crate) mod model;

pub(crate) use self::http::router;
