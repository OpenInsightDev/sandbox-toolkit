//! The `/process` resource API.
//!
//! exec and shell run over a stream and share the length-prefixed frames in
//! [`frame`], while a pty session runs over a WebSocket and uses the per-message
//! frames in [`pty`]. The wire types exported to TypeScript clients live in
//! [`model`].

mod frame;
mod model;
mod pty;
