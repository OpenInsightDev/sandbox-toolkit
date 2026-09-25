//! A workspace registers a remote absolute path under a caller-chosen id and is
//! the boundary every path-bearing operation is confined to.

mod http;
mod mcp;
pub(crate) mod model;
pub(crate) mod registry;

pub(crate) use self::http::router;
pub(crate) use self::registry::WorkspaceRegistry;
