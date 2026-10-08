//! Parse the resources a plugin holds inside `.agents/`.
//!
//! The crate is split along the resource types the specification defines:
//! [`mcp`] parses an `mcp.json`, [`skill`] parses a `SKILL.md`, and [`plugin`]
//! composes both while loading a whole plugin. [`DotAgents`] watches a whole
//! `.agents/` directory — its own components and the plugins under `plugins/`
//! — and reports how it changed as [`watch::Event`]s. Each resource is available
//! in two forms: a pure `parse` over bytes, and an async `load` that reads the
//! file through `tokio::fs`.
//!
//! The models and the parsing rules mirror the reference implementation,
//! [agent-plugin-rs], with two deliberate differences: every model is
//! `Hash`able, so the values can key a map or join a set, and file access goes
//! through `tokio::fs` instead of blocking `std::fs`.
//!
//! # Loading a plugin
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
//! # Loading a `.agents` directory
//!
//! ```no_run
//! # async fn example() -> Result<(), dotagents::watch::Error> {
//! let agents = dotagents::DotAgents::open(".agents").await?;
//! let mut events = agents.subscribe();
//!
//! loop {
//!     if let Ok(event) = events.recv().await {
//!         println!("changed: {event:?}");
//!     }
//!     match agents.state() {
//!         Ok(resources) => println!("{} plugin(s)", resources.plugins.len()),
//!         Err(error) => println!("no resources: {error}"),
//!     }
//! }
//! # }
//! ```
//!
//! [Agent Plugins]: https://agent-plugins.org/
//! [agent-plugin-rs]: https://github.com/Toasterson/agent-plugin-rs

mod agents;
mod diag;
mod name;
mod path;
mod spec;
mod template;

pub mod mcp;
pub mod plugin;
pub mod skill;
pub mod watch;

pub use agents::{DotAgents, LoadError, Resources};
pub use diag::{Diagnostic, Origin, Rule};
pub use name::{InvalidName, PluginName};
pub use path::{PackagePath, RelativePath, RelativePathError};
pub use spec::SpecVersion;
pub use template::{Anchors, Placeholder, Segment, Template};
