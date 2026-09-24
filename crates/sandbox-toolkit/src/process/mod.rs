//! The `/process` resource API.
//!
//! exec and shell run over HTTP and share the length-prefixed frames in
//! [`exec`], while a pty session runs over a WebSocket and uses the per-message
//! frames in [`pty`]. The wire types exported to TypeScript clients live in
//! [`model`].

mod exec;
mod http;
mod model;
mod pty;

pub(crate) use self::http::router;
