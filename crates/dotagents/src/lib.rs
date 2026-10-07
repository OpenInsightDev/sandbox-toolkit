//! Parse the resources an [Agent Plugins] package holds inside `.agents/`.
//!
//! The crate is split along the resource types the specification defines:
//! [`mcp`] parses an `mcp.json`, [`skill`] parses a `SKILL.md`, and [`plugin`]
//! composes both while loading a whole package. Each resource is available in
//! two forms: a pure `parse` over bytes, and an async `load` that reads the
//! file through `tokio::fs`.
//!
//! The models and the parsing rules mirror the reference implementation,
//! [agent-plugin-rs], with two deliberate differences: every model is
//! `Hash`able, so the values can key a map or join a set, and file access goes
//! through `tokio::fs` instead of blocking `std::fs`.
//!
//! # Loading a package
//!
//! ```no_run
//! # async fn example() -> Result<(), dotagents::plugin::Rejection> {
//! let plugin = dotagents::plugin::load(".agents").await?;
//!
//! println!("{} has {} skill(s)", plugin.manifest.name, plugin.skills.len());
//! for skill in &plugin.skills {
//!     println!("  {}", skill.meta.name);
//! }
//! # Ok(())
//! # }
//! ```
//!
//! [Agent Plugins]: https://agent-plugins.org/
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
