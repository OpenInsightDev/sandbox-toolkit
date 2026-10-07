//! The observation a [`Watcher`](super::Watcher) diffs against.
//!
//! Changes are derived by reloading the package and comparing typed state, not
//! by reading `notify`'s rename or create/remove kinds: a rename is just the
//! old identity disappearing and a new one appearing, wherever it happened.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::diag::{Diagnostic, Origin, Rule};
use crate::mcp::ServerEntry;
use crate::name::PluginName;
use crate::plugin::{self, LoadedPlugin, Manifest, ManifestRejection, McpOutcome, Rejection};
use crate::skill::Skill;

use super::event::{Event, McpChange, PluginChange, SkillChange};

/// The package as of the last state the watcher reported.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Snapshot {
    manifest: ManifestState,
    skills: BTreeMap<String, SkillState>,
    mcp: McpState,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum ManifestState {
    #[default]
    Missing,
    Invalid(ManifestRejection),
    Loaded(Box<Manifest>),
}

impl ManifestState {
    fn name(&self) -> Option<&PluginName> {
        match self {
            Self::Loaded(manifest) => Some(&manifest.name),
            Self::Missing | Self::Invalid(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SkillState {
    Valid(Box<Skill>),
    Invalid(Vec<Diagnostic>),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum McpState {
    #[default]
    Absent,
    Invalid(Vec<Diagnostic>),
    Configured(Vec<ServerEntry>),
}

impl McpState {
    /// Whether the state still holds an `mcp.json` component at all.
    fn held(&self) -> bool {
        !matches!(self, Self::Absent)
    }
}

/// Load the package and reduce it to the state the diff compares.
///
/// A rejected package discovers no components, so a collapse yields no
/// per-resource observations — that is why the plugin event carries the flags.
pub(super) async fn observe(root: &Path) -> Snapshot {
    match plugin::load(root).await {
        Ok(loaded) => from_loaded(loaded),
        Err(Rejection::Manifest(rejection)) => Snapshot {
            manifest: ManifestState::Invalid(rejection),
            ..Snapshot::default()
        },
        // No usable manifest, or the IO layer never got that far.
        Err(_) => Snapshot::default(),
    }
}

fn from_loaded(loaded: LoadedPlugin) -> Snapshot {
    let LoadedPlugin { manifest, skills, mcp, diagnostics } = loaded;

    let mut skill_states: BTreeMap<String, SkillState> = BTreeMap::new();
    for skill in skills {
        let id = skill.directory.clone();
        skill_states.insert(id, SkillState::Valid(Box::new(skill)));
    }
    let mut invalid: BTreeMap<String, Vec<Diagnostic>> = BTreeMap::new();
    for diagnostic in diagnostics {
        if let Origin::Skill(id) = &diagnostic.origin {
            invalid.entry(id.clone()).or_default().push(diagnostic);
        }
    }
    for (id, diagnostics) in invalid {
        skill_states.insert(id, SkillState::Invalid(diagnostics));
    }

    let mcp = match mcp {
        McpOutcome::Absent => McpState::Absent,
        McpOutcome::Disabled(reason) => McpState::Invalid(vec![Diagnostic::new(
            Rule::McpDisabled,
            Origin::Mcp,
            reason.to_string(),
        )]),
        McpOutcome::Configured(config) => McpState::Configured(config.servers),
    };

    Snapshot { manifest: ManifestState::Loaded(Box::new(manifest)), skills: skill_states, mcp }
}

/// The events that turn `previous` into `current`.
pub(super) fn diff(previous: &Snapshot, current: &Snapshot) -> Vec<Event> {
    let mut events = Vec::new();
    diff_manifest(previous, current, &mut events);

    // A rejected package holds no components, so it produces none of these
    // events; the plugin event already told the consumer to drop them.
    if current.manifest.name().is_some() {
        let plugin = current.manifest.name().cloned();
        diff_skills(previous, current, &plugin, &mut events);
        diff_mcp(previous, current, &plugin, &mut events);
    }

    events
}

fn diff_manifest(previous: &Snapshot, current: &Snapshot, events: &mut Vec<Event>) {
    use ManifestState::{Invalid, Loaded, Missing};

    let held_skills = !previous.skills.is_empty();
    let held_mcp = previous.mcp.held();
    let old_name = previous.manifest.name().cloned();

    match (&previous.manifest, &current.manifest) {
        (Missing, Missing) => {}
        (Loaded(a), Loaded(b)) if a == b => {}
        (Loaded(a), Loaded(b)) if a.name == b.name => {
            events.push(plugin_event(
                Some(b.name.clone()),
                PluginChange::Modified(b.as_ref().clone()),
            ));
        }
        // A name change is the old identity retiring and the new one arriving.
        (Loaded(a), Loaded(b)) => {
            events.push(plugin_event(
                Some(a.name.clone()),
                PluginChange::Removed { skills: held_skills, mcp: held_mcp },
            ));
            events.push(plugin_event(
                Some(b.name.clone()),
                PluginChange::Added(b.as_ref().clone()),
            ));
        }
        (Loaded(a), Missing) => {
            events.push(plugin_event(
                Some(a.name.clone()),
                PluginChange::Removed { skills: held_skills, mcp: held_mcp },
            ));
        }
        (Loaded(a), Invalid(rejection)) => {
            events.push(plugin_event(
                Some(a.name.clone()),
                PluginChange::Invalid {
                    rejection: rejection.clone(),
                    skills: held_skills,
                    mcp: held_mcp,
                },
            ));
        }
        (Missing | Invalid(_), Loaded(b)) => {
            events.push(plugin_event(
                Some(b.name.clone()),
                PluginChange::Added(b.as_ref().clone()),
            ));
        }
        (Invalid(a), Invalid(b)) if a == b => {}
        (Missing, Invalid(rejection)) => {
            events.push(plugin_event(
                None,
                PluginChange::Invalid {
                    rejection: rejection.clone(),
                    skills: held_skills,
                    mcp: held_mcp,
                },
            ));
        }
        (Invalid(_), Invalid(rejection)) => {
            events.push(plugin_event(
                old_name,
                PluginChange::Invalid {
                    rejection: rejection.clone(),
                    skills: held_skills,
                    mcp: held_mcp,
                },
            ));
        }
        (Invalid(_), Missing) => {
            events.push(plugin_event(old_name, PluginChange::Removed {
                skills: held_skills,
                mcp: held_mcp,
            }));
        }
    }
}

fn diff_skills(
    previous: &Snapshot,
    current: &Snapshot,
    plugin: &Option<PluginName>,
    events: &mut Vec<Event>,
) {
    let ids: BTreeSet<&String> = previous.skills.keys().chain(current.skills.keys()).collect();
    for id in ids {
        let before = previous.skills.get(id);
        let after = current.skills.get(id);
        let change = match (before, after) {
            (Some(a), Some(b)) if a == b => continue,
            (_, Some(SkillState::Valid(skill))) => match before {
                None | Some(SkillState::Invalid(_)) => SkillChange::Added(skill.as_ref().clone()),
                Some(SkillState::Valid(_)) => SkillChange::Modified(skill.as_ref().clone()),
            },
            (_, Some(SkillState::Invalid(diagnostics))) => {
                SkillChange::Invalid(diagnostics.clone())
            }
            (Some(_), None) => SkillChange::Removed,
            (None, None) => unreachable!(),
        };
        events.push(Event::Skill { plugin: plugin.clone(), id: id.clone(), change });
    }
}

fn diff_mcp(
    previous: &Snapshot,
    current: &Snapshot,
    plugin: &Option<PluginName>,
    events: &mut Vec<Event>,
) {
    let before = &previous.mcp;
    let after = &current.mcp;
    if before == after {
        return;
    }

    let change = match (before, after) {
        (McpState::Configured(old), McpState::Configured(new)) => McpChange::Reloaded {
            added: only_in(new, old),
            removed: only_in(old, new),
        },
        (McpState::Configured(old), McpState::Absent) => {
            McpChange::Reloaded { added: Vec::new(), removed: old.clone() }
        }
        (_, McpState::Invalid(diagnostics)) => McpChange::Invalid(diagnostics.clone()),
        (_, McpState::Configured(new)) => {
            McpChange::Reloaded { added: new.clone(), removed: Vec::new() }
        }
        // Disabling an already-disabled component changes no server set.
        (McpState::Absent | McpState::Invalid(_), McpState::Absent) => return,
    };

    events.push(Event::Mcp { plugin: plugin.clone(), change });
}

fn plugin_event(plugin: Option<PluginName>, change: PluginChange) -> Event {
    Event::Plugin { plugin, change }
}

fn only_in(entries: &[ServerEntry], other: &[ServerEntry]) -> Vec<ServerEntry> {
    entries.iter().filter(|entry| !other.contains(entry)).cloned().collect()
}
