//! Parse the resources a plugin holds inside `.agents/`.
//!
//! The models and the parsing rules mirror the reference implementation,
//! [agent-plugin-rs], with two deliberate differences: every model is
//! `Hash`able, so the values can key a map or join a set, and file access goes
//! through `tokio::fs` instead of blocking `std::fs`.
//!
//! [agent-plugin-rs]: https://github.com/Toasterson/agent-plugin-rs

mod diag;
mod name;
mod path;
mod spec;
mod template;

pub mod mcp;
pub mod plugin;
pub mod skill;

pub use diag::{Diagnostic, Origin, Rule};
pub use name::{InvalidName, PluginName};
pub use path::{PackagePath, RelativePath, RelativePathError};
pub use spec::SpecVersion;
pub use template::{Anchors, Placeholder, Segment, Template};
