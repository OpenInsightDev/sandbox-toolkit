//! The changes a watched `.agents/` directory produces.
//!
//! Each `change` variant carries exactly the content its transition has: the
//! resource when it is valid, and nothing when it is gone. A directory that
//! stops holding resources at all — it disappeared, or it stopped loading — is
//! one [`Event::Invalid`] carrying the reason, rather than a removal per
//! resource.
//!
//! Changes are derived from the content of two consecutive loads rather than
//! from filesystem event kinds, so renames, atomic saves, and broken content
//! all behave the same.

use std::collections::BTreeSet;
use std::fmt;

use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;

use crate::agents::{LoadError, Resources, Snapshot};
use crate::mcp;
use crate::plugin;
use crate::skill;

pub use sandbox_toolkit_utils::watch::Error;

pub(crate) const EVENT_CAPACITY: usize = 256;

/// One change inside a watched `.agents/` directory.
///
/// A plugin carries its own skills and servers, so they change with it, as
/// [`PluginChange`]. [`Event::Skill`] and [`Event::Mcp`] are the directory's
/// own `skills/` and `mcp.json`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Event {
    /// The directory as a whole holds no resources — it is missing, or it does
    /// not load — so everything a subscriber held is gone.
    Invalid(LoadError),
    /// One plugin under `plugins/`, by its directory name.
    Plugin {
        /// The plugin's directory name.
        id: String,
        /// How it changed.
        change: PluginChange,
    },
    /// One skill under `skills/`, by its directory name.
    Skill {
        /// The skill directory's name.
        id: String,
        /// How it changed.
        change: SkillChange,
    },
    /// The `mcp.json` server set changed; at least one side is non-empty. A
    /// server whose configuration changed appears on both sides.
    Mcp {
        /// Servers that were not present before.
        added: Vec<mcp::ServerEntry>,
        /// Servers that are no longer present.
        removed: Vec<mcp::ServerEntry>,
    },
}

/// How one plugin changed.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PluginChange {
    /// The plugin entered the set.
    Added(plugin::Plugin),
    /// A plugin's content changed; its skills and servers came with it.
    Modified(plugin::Plugin),
    /// The plugin left the set.
    Removed,
}

/// How one skill changed.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SkillChange {
    /// The skill entered the valid set.
    Added(skill::Skill),
    /// A valid skill's content changed.
    Modified(skill::Skill),
    /// The skill disappeared.
    Removed,
}

/// A subscriber's view of the changes after it subscribed.
pub struct Subscription(broadcast::Receiver<Event>);

impl Subscription {
    pub(crate) fn new(receiver: broadcast::Receiver<Event>) -> Self {
        Self(receiver)
    }

    /// Await the next change.
    ///
    /// An error means changes were lost or the watch stopped; either way the
    /// subscriber resynchronises by reading
    /// [`DotAgents::state`](crate::DotAgents::state).
    pub async fn recv(&mut self) -> Result<Event, SubscriptionError> {
        match self.0.recv().await {
            Ok(event) => Ok(event),
            Err(RecvError::Lagged(events)) => Err(SubscriptionError::Lagged { events }),
            Err(RecvError::Closed) => Err(SubscriptionError::Stopped),
        }
    }
}

/// Why a subscription carried no change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SubscriptionError {
    /// The subscriber fell behind and lost this many changes.
    Lagged {
        /// How many changes were dropped.
        events: u64,
    },
    /// The watched directory is no longer watched.
    Stopped,
}

impl fmt::Display for SubscriptionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lagged { events } => {
                write!(f, "subscription fell behind and lost {events} change(s)")
            }
            Self::Stopped => f.write_str("the watched directory is no longer watched"),
        }
    }
}

impl std::error::Error for SubscriptionError {}

/// The events that turn `previous` into `current`.
pub(crate) fn diff(previous: &Snapshot, current: &Snapshot) -> Vec<Event> {
    if previous == current {
        return Vec::new();
    }
    match (previous, current) {
        (Ok(before), Ok(after)) => diff_resources(before, after),
        // Holding nothing now: the set is void, so one event says so rather
        // than a removal per resource that used to be there.
        (_, Err(error)) => vec![Event::Invalid(error.clone())],
        // Holding resources now: everything arrived.
        (Err(_), Ok(after)) => diff_resources(&nothing(), after),
    }
}

/// A resource a `.agents/` directory holds under a name, so two loads can be
/// compared entry by entry.
trait Named: PartialEq {
    /// The name the resource is held under.
    fn name(&self) -> &str;
}

impl Named for plugin::Plugin {
    fn name(&self) -> &str {
        &self.id
    }
}

impl Named for skill::Skill {
    fn name(&self) -> &str {
        &self.directory
    }
}

/// How one resource differs between two loads.
enum Difference<'a, T> {
    /// The resource is new.
    Added(&'a T),
    /// The resource is there on both sides, but changed.
    Modified(&'a T),
    /// The resource is gone.
    Removed,
}

impl From<Difference<'_, plugin::Plugin>> for PluginChange {
    fn from(difference: Difference<'_, plugin::Plugin>) -> Self {
        match difference {
            Difference::Added(plugin) => Self::Added(plugin.clone()),
            Difference::Modified(plugin) => Self::Modified(plugin.clone()),
            Difference::Removed => Self::Removed,
        }
    }
}

impl From<Difference<'_, skill::Skill>> for SkillChange {
    fn from(difference: Difference<'_, skill::Skill>) -> Self {
        match difference {
            Difference::Added(skill) => Self::Added(skill.clone()),
            Difference::Modified(skill) => Self::Modified(skill.clone()),
            Difference::Removed => Self::Removed,
        }
    }
}

/// The resources that differ between two loads, by name, in name order.
fn differences<'a, T: Named>(
    before: &'a [T],
    after: &'a [T],
) -> impl Iterator<Item = (&'a str, Difference<'a, T>)> {
    let names: BTreeSet<&str> = before.iter().chain(after).map(|entry| entry.name()).collect();
    names.into_iter().filter_map(move |name| {
        let old = before.iter().find(|entry| entry.name() == name);
        let new = after.iter().find(|entry| entry.name() == name);
        if old == new {
            return None;
        }
        let difference = match new {
            Some(entry) if old.is_none() => Difference::Added(entry),
            Some(entry) => Difference::Modified(entry),
            None => Difference::Removed,
        };
        Some((name, difference))
    })
}

/// The events that turn one load's resources into the next load's.
fn diff_resources(before: &Resources, after: &Resources) -> Vec<Event> {
    let plugins = differences(&before.plugins, &after.plugins)
        .map(|(id, difference)| Event::Plugin { id: id.to_owned(), change: difference.into() });
    let skills = differences(&before.skills, &after.skills)
        .map(|(id, difference)| Event::Skill { id: id.to_owned(), change: difference.into() });

    plugins.chain(skills).chain(diff_mcp(&before.mcps, &after.mcps)).collect()
}

/// The `mcp.json` server set, when it differs. A server whose configuration
/// changed appears on both sides.
fn diff_mcp(before: &[mcp::ServerEntry], after: &[mcp::ServerEntry]) -> Option<Event> {
    let added = only_in(after, before);
    let removed = only_in(before, after);
    if added.is_empty() && removed.is_empty() {
        return None;
    }
    Some(Event::Mcp { added, removed })
}

/// A directory that holds nothing, so diffing against it reports every
/// resource as arriving.
fn nothing() -> Resources {
    Resources { plugins: Vec::new(), skills: Vec::new(), mcps: Vec::new(), diagnostics: Vec::new() }
}

fn only_in(entries: &[mcp::ServerEntry], other: &[mcp::ServerEntry]) -> Vec<mcp::ServerEntry> {
    entries.iter().filter(|entry| !other.contains(entry)).cloned().collect()
}
