use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use bytes::Bytes;
use notify::EventKind;
use sandbox_toolkit_utils::watch::Watch;
use serde::Serialize;
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;

use crate::mcp;
use crate::path::{AGENTS_DIR, MCP_JSON, PLUGINS_DIR, PLUGIN_JSON, SKILLS_DIR, SKILL_MD};
use crate::plugin::{Plugin, Plugins};
use crate::skill::{Skill, Skills};

const CAPACITY: usize = 256;

/// A source file's content identity. A rewrite changes it and nothing else
/// does, so a change to another file in the same directory reports nothing.
pub type Digest = [u8; 32];

/// The identity of a source discovery did not hold.
pub const ABSENT: Digest = [0; 32];

/// The identity of `bytes`, which discovery computes while it already holds the
/// file's content.
pub fn digest(bytes: &[u8]) -> Digest {
    *blake3::hash(bytes).as_bytes()
}

/// One resource's ids, each with the identity of the file it was discovered
/// from.
pub type Sources = BTreeMap<String, Digest>;

/// The resource a lifecycle event belongs to. `Workspace` is a registry entry
/// rather than something a scope holds, so only `/workspaces` reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resource {
    Skill,
    Plugin,
    Mcp,
    Workspace,
}

impl Resource {
    pub fn name(self) -> &'static str {
        match self {
            Self::Skill => "skill",
            Self::Plugin => "plugin",
            Self::Mcp => "mcp",
            Self::Workspace => "workspace",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Register,
    Unregister,
    Update,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Register => "register",
            Self::Unregister => "unregister",
            Self::Update => "update",
        }
    }
}

/// One id entering, leaving, or changing within a resource's discovered set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub resource: Resource,
    pub kind: Kind,
    pub id: String,
}

impl Event {
    pub fn new(resource: Resource, kind: Kind, id: impl Into<String>) -> Self {
        Self {
            resource,
            kind,
            id: id.into(),
        }
    }
}

/// A broadcaster for the events of one source: a scope's observer, or the
/// workspace registry.
pub fn channel() -> broadcast::Sender<Event> {
    broadcast::channel(CAPACITY).0
}

/// The events a mount reports. A scope that holds no resources has no observer,
/// and reports none.
pub fn subscribe(observer: Option<&Observer>) -> broadcast::Receiver<Event> {
    match observer {
        Some(observer) => observer.subscribe(),
        None => silent(),
    }
}

/// A stream that carries nothing and stays open.
pub fn silent() -> broadcast::Receiver<Event> {
    static SILENT: OnceLock<broadcast::Sender<Event>> = OnceLock::new();

    SILENT.get_or_init(channel).subscribe()
}

/// One event as an SSE frame: its `event:` name, the `data:` object a resource's
/// own mount carries, and the blank line that ends it.
pub fn resource_frame(event: &Event) -> Bytes {
    frame(event, &Identified { id: &event.id })
}

/// The same frame with the `resource` a summary's `data` object adds, so a new
/// resource needs no change there.
pub fn summary_frame(event: &Event) -> Bytes {
    frame(
        event,
        &Summary {
            resource: event.resource.name(),
            id: &event.id,
        },
    )
}

fn frame(event: &Event, data: &impl Serialize) -> Bytes {
    let data = serde_json::to_string(data).expect("serialize an event");

    Bytes::from(format!("event: {}\ndata: {data}\n\n", event.kind.name()))
}

#[derive(Serialize)]
struct Identified<'a> {
    id: &'a str,
}

#[derive(Serialize)]
struct Summary<'a> {
    resource: &'a str,
    id: &'a str,
}

/// A scope's watcher: it tracks the resource sets of one workspace on disk and
/// reports what a change did to them. Every stream of that scope subscribes to
/// the one broadcaster, so an event reaches all of them.
pub struct Observer {
    events: broadcast::Sender<Event>,
    _watch: Watch,
}

impl Observer {
    /// `plugins` and `mcps` are the scope's resources as it was constructed: the
    /// baseline the first change is measured against.
    pub async fn new(
        scope: &str,
        root: &Path,
        plugins: Plugins,
        mcps: Arc<mcp::Runtime>,
    ) -> Result<Self, sandbox_toolkit_utils::watch::Error> {
        let watched = root.join(AGENTS_DIR);
        let watch = Watch::recursive(&watched).await?;
        let mut changed = watch.subscribe();
        let mut tracker = Tracker::baseline(scope, root, plugins, mcps).await;

        let events = channel();
        let task_events = events.clone();
        tokio::spawn(async move {
            loop {
                match changed.recv().await {
                    Ok(Ok(event)) => {
                        let mut affected = affects(&watched, &event);
                        // A burst of events — a directory and the file that lands
                        // in it — is one change: settle once for all of it.
                        while let Ok(Ok(more)) = changed.try_recv() {
                            affected = affected.union(affects(&watched, &more));
                        }

                        if affected.is_none() {
                            continue;
                        }

                        for event in tracker.settle(affected).await {
                            let _ = task_events.send(event);
                        }
                    }
                    // A watcher error, or a batch this observer outran, is not a
                    // reason to stop reporting later changes.
                    Ok(Err(_)) | Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => break,
                }
            }
        });

        Ok(Self {
            events,
            _watch: watch,
        })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }
}

/// What a change can affect: the sets a mount discovers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Affected {
    plugins: bool,
    skills: bool,
    mcps: bool,
}

impl Affected {
    fn nothing() -> Self {
        Self {
            plugins: false,
            skills: false,
            mcps: false,
        }
    }

    /// A plugin package changed. The skills and entries it contributes are
    /// discovered from it, so they are recomputed with it.
    fn plugins() -> Self {
        Self {
            plugins: true,
            skills: true,
            mcps: true,
        }
    }

    fn skills() -> Self {
        Self {
            skills: true,
            ..Self::nothing()
        }
    }

    fn mcps() -> Self {
        Self {
            mcps: true,
            ..Self::nothing()
        }
    }

    /// The watched directory itself, or a name this build cannot place: the
    /// whole scope is rescanned rather than risk missing a set.
    fn everything() -> Self {
        Self::plugins()
    }

    fn is_none(self) -> bool {
        !self.plugins && !self.skills && !self.mcps
    }

    fn union(self, other: Self) -> Self {
        Self {
            plugins: self.plugins || other.plugins,
            skills: self.skills || other.skills,
            mcps: self.mcps || other.mcps,
        }
    }
}

/// What one watched event can affect, from the paths it reports. An access or a
/// metadata-only event changes no content, so it affects nothing.
fn affects(watched: &Path, event: &notify::Event) -> Affected {
    if !rewritten(&event.kind) {
        return Affected::nothing();
    }

    event.paths.iter().fold(Affected::nothing(), |affected, path| {
        affected.union(path_affects(watched, path))
    })
}

/// What a changed path can affect. Every file the sets are discovered from is
/// named here; anything else a skill or package directory holds belongs to no
/// set, so a change to it reports nothing and rescans nothing.
fn path_affects(watched: &Path, path: &Path) -> Affected {
    let Ok(relative) = path.strip_prefix(watched) else {
        return Affected::everything();
    };
    // A name that is not text names no source.
    let Some(segments) = relative.iter().map(OsStr::to_str).collect::<Option<Vec<_>>>() else {
        return Affected::nothing();
    };

    match segments.as_slice() {
        [MCP_JSON] => Affected::mcps(),
        [SKILLS_DIR] | [SKILLS_DIR, _] | [SKILLS_DIR, _, SKILL_MD] => Affected::skills(),
        // A package's manifest, its own entries, and its skills.
        [PLUGINS_DIR]
        | [PLUGINS_DIR, _]
        | [PLUGINS_DIR, _, PLUGIN_JSON | MCP_JSON]
        | [PLUGINS_DIR, _, SKILLS_DIR]
        | [PLUGINS_DIR, _, SKILLS_DIR, _]
        | [PLUGINS_DIR, _, SKILLS_DIR, _, SKILL_MD] => Affected::plugins(),
        [] => Affected::everything(),
        _ => Affected::nothing(),
    }
}

struct Tracker {
    scope: String,
    root: PathBuf,
    /// The scope's packages as the last scan found them; `None` after a package
    /// the specification rejects, which leaves the scope holding nothing until
    /// one loads again.
    plugins: Option<Plugins>,
    mcps: Arc<mcp::Runtime>,
    previous: Sets,
}

impl Tracker {
    /// The sets the scope already holds, so only later changes are reported.
    async fn baseline(scope: &str, root: &Path, plugins: Plugins, mcps: Arc<mcp::Runtime>) -> Self {
        let skills = Skills::new(scope, root, plugins.clone());
        let previous = Sets {
            plugins: plugin_identities(plugins.list()),
            skills: skill_identities(&skills.list().await),
            mcps: mcps.sources().await,
        };

        Self {
            scope: scope.to_owned(),
            root: root.to_path_buf(),
            plugins: Some(plugins),
            mcps,
            previous,
        }
    }

    /// Rescans what `affected` names and reports what changed since the last
    /// settle.
    async fn settle(&mut self, affected: Affected) -> Vec<Event> {
        let current = self.scan(affected).await;
        let mut events = Vec::new();
        diff(
            Resource::Skill,
            &self.previous.skills,
            &current.skills,
            &mut events,
        );
        diff(
            Resource::Plugin,
            &self.previous.plugins,
            &current.plugins,
            &mut events,
        );
        diff(
            Resource::Mcp,
            &self.previous.mcps,
            &current.mcps,
            &mut events,
        );
        self.previous = current;

        events
    }

    /// The scope's sets after a change. Only what the change affects is
    /// recomputed; the other sets keep the ids and identities the last scan
    /// found.
    async fn scan(&mut self, affected: Affected) -> Sets {
        if affected.plugins {
            self.plugins = match Plugins::load(&self.root) {
                Ok(plugins) => Some(plugins),
                Err(error) => {
                    tracing::warn!(%error, "failed to rediscover plugins");
                    None
                }
            };
        }

        // A package that stopped loading takes the whole set with it: the scope
        // holds nothing until one loads again.
        let Some(plugins) = &self.plugins else {
            self.mcps.reload(None).await;

            return Sets::default();
        };

        let mut next = self.previous.clone();
        if affected.plugins {
            next.plugins = plugin_identities(plugins.list());
        }
        if affected.skills {
            let skills = Skills::new(self.scope.as_str(), self.root.as_path(), plugins.clone());
            next.skills = skill_identities(&skills.list().await);
        }
        if affected.mcps {
            next.mcps = self.mcps.reload(Some(plugins)).await;
        }

        next
    }
}

#[derive(Clone, Default)]
struct Sets {
    skills: Sources,
    plugins: Sources,
    mcps: Sources,
}

/// What one resource's set reports from `previous` to `current`: an id that
/// left is an `unregister`, one that arrived a `register`, and one whose source
/// was rewritten an `update`.
fn diff(resource: Resource, previous: &Sources, current: &Sources, events: &mut Vec<Event>) {
    for id in previous.keys() {
        if !current.contains_key(id) {
            events.push(Event::new(resource, Kind::Unregister, id.clone()));
        }
    }

    for (id, digest) in current {
        match previous.get(id) {
            None => events.push(Event::new(resource, Kind::Register, id.clone())),
            Some(was) if was != digest => {
                events.push(Event::new(resource, Kind::Update, id.clone()))
            }
            Some(_) => {}
        }
    }
}

/// Each plugin's directory name, with the `plugin.json` it was loaded from.
fn plugin_identities(plugins: &[Plugin]) -> Sources {
    plugins
        .iter()
        .map(|plugin| (plugin.id.clone(), plugin.digest))
        .collect()
}

/// Each skill's id, with the `SKILL.md` it was discovered from.
fn skill_identities(skills: &[Skill]) -> Sources {
    skills
        .iter()
        .map(|skill| (skill.id.clone(), skill.digest))
        .collect()
}

/// Whether an event could have changed what the scope holds; an access or a
/// metadata-only event cannot.
fn rewritten(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `.agents` directory an observer is rooted at.
    fn agents() -> PathBuf {
        PathBuf::from("/ws/.agents")
    }

    /// What a change to `path` within the scope affects.
    fn affects_path(path: &str) -> Affected {
        path_affects(&agents(), &agents().join(path))
    }

    #[test]
    fn a_scope_owns_its_own_entries_and_skills() {
        assert_eq!(affects_path("mcp.json"), Affected::mcps());
        assert_eq!(affects_path("skills"), Affected::skills());
        assert_eq!(affects_path("skills/deploy"), Affected::skills());
        assert_eq!(affects_path("skills/deploy/SKILL.md"), Affected::skills());
    }

    #[test]
    fn a_package_carries_its_declared_skills_and_entries() {
        for path in [
            "plugins",
            "plugins/deploy-kit",
            "plugins/deploy-kit/plugin.json",
            "plugins/deploy-kit/mcp.json",
            "plugins/deploy-kit/skills",
            "plugins/deploy-kit/skills/deploy",
            "plugins/deploy-kit/skills/deploy/SKILL.md",
        ] {
            assert_eq!(affects_path(path), Affected::plugins(), "{path}");
        }
    }

    #[test]
    fn a_file_no_set_is_discovered_from_affects_nothing() {
        for path in [
            "other.json",
            // Another file in a skill or a package, and a `SKILL.md` no set
            // reads: only a skill directory's own file is one.
            "skills/deploy/run.sh",
            "skills/deploy/notes/SKILL.md",
            "plugins/deploy-kit/notes.txt",
            "plugins/deploy-kit/skills/deploy/notes/SKILL.md",
        ] {
            assert_eq!(affects_path(path), Affected::nothing(), "{path}");
        }
    }

    #[test]
    fn a_change_the_scope_cannot_place_rescans_everything() {
        assert_eq!(path_affects(&agents(), &agents()), Affected::everything());
        assert_eq!(
            path_affects(&agents(), Path::new("/elsewhere/SKILL.md")),
            Affected::everything()
        );
    }

    #[test]
    fn a_burst_of_changes_affects_what_its_paths_do() {
        assert_eq!(
            Affected::skills().union(Affected::mcps()),
            Affected {
                plugins: false,
                skills: true,
                mcps: true,
            }
        );
        assert_eq!(
            Affected::mcps().union(Affected::nothing()),
            Affected::mcps()
        );
    }
}
