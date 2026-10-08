//! The `.agents/` directory as a whole.
//!
//! A plugin is one directory that carries a `plugin.json`. A `.agents/`
//! directory carries several resource classes beside each other: its own skills
//! and servers, and the plugins under `plugins/`, each with their own.
//! [`DotAgents`] watches the directory, hands out one consistent [`Resources`],
//! and reports how it changed as [`watch::Event`]s.
//!
//! The directory's own `skills/` and `mcp.json` are the component locations a
//! plugin also has, so they load through the same rules — only the manifest is
//! absent, which is why `mcp.json` here selects its specification version
//! directly rather than from one.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use notify::EventKind;
use sandbox_toolkit_utils::watch::{Watch, WatchEvent};
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;

use crate::diag::Diagnostic;
use crate::mcp;
use crate::plugin;
use crate::skill;
use crate::spec::SpecVersion;
use crate::watch;

const PLUGINS_DIR: &str = "plugins";

/// One watched `.agents/` directory.
///
/// Dropping it stops the watch.
pub struct DotAgents {
    state: tokio::sync::watch::Receiver<Snapshot>,
    events: broadcast::Sender<watch::Event>,
}

/// The directory as one value: its resources, or why it holds none.
pub(crate) type Snapshot = Result<Arc<Resources>, LoadError>;

/// Every resource one `.agents/` directory holds.
///
/// `skills` and `mcps` are the directory's *own* resources; a plugin's stay
/// inside its [`plugin::Plugin`], where a skill's directory name is its id.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Resources {
    /// The plugins under `plugins/`, in directory-name order.
    pub plugins: Vec<plugin::Plugin>,
    /// The skills under `skills/`, in directory-name order.
    pub skills: Vec<skill::Skill>,
    /// The servers `mcp.json` declares, in declaration order.
    pub mcps: Vec<mcp::ServerEntry>,
    /// Everything non-fatal the load decided about the directory's own
    /// components.
    pub diagnostics: Vec<Diagnostic>,
}

/// Why a `.agents/` directory holds nothing.
///
/// A load is whole, as the specification requires of a resource collection: any
/// class that fails to load leaves the directory with no resources at all.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum LoadError {
    /// There is no `.agents/` directory at the watched path.
    Absent,
    /// A directory could not be read.
    Unavailable {
        /// The IO layer's explanation.
        detail: String,
    },
    /// A directory under `plugins/` was rejected.
    PluginRejected {
        /// The directory that was rejected.
        directory: PathBuf,
        /// The fatal problem.
        rejection: plugin::Rejection,
    },
    /// A plugin under `plugins/` has its MCP component disabled.
    PluginMcpDisabled {
        /// The plugin's id.
        id: String,
        /// Why MCP was disabled.
        reason: mcp::DisabledReason,
    },
    /// The directory's own `mcp.json` does not validate.
    Mcp(mcp::DisabledReason),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Absent => f.write_str("there is no `.agents` directory"),
            Self::Unavailable { detail } => write!(f, "`.agents` could not be read: {detail}"),
            Self::PluginRejected { directory, rejection } => {
                write!(f, "the plugin at `{}` was rejected: {rejection}", directory.display())
            }
            Self::PluginMcpDisabled { id, reason } => {
                write!(f, "plugin `{id}` has its MCP component disabled: {reason}")
            }
            Self::Mcp(reason) => write!(f, "`mcp.json` is disabled: {reason}"),
        }
    }
}

impl std::error::Error for LoadError {}

impl DotAgents {
    /// Watch the `.agents/` directory at `root`.
    ///
    /// The baseline is established after the watch is live, so no change slips
    /// between the two and a subscriber only ever sees later ones.
    pub async fn open(root: impl AsRef<Path>) -> Result<Self, watch::Error> {
        let root = root.as_ref().to_path_buf();
        let watcher = Watch::recursive(&root).await?;
        let source = watcher.subscribe();

        let state = observe(&root).await;
        let (current, receiver) = tokio::sync::watch::channel(state);
        let (events, _) = broadcast::channel(watch::EVENT_CAPACITY);

        tokio::spawn(drive(root, watcher, source, current, events.clone()));
        Ok(Self { state: receiver, events })
    }

    /// The resources the directory holds right now, or why it holds none.
    ///
    /// One snapshot, so a caller reading several fields sees a single moment.
    pub fn state(&self) -> Result<Arc<Resources>, LoadError> {
        self.state.borrow().clone()
    }

    /// Subscribe to the changes from here on.
    pub fn subscribe(&self) -> watch::Subscription {
        watch::Subscription::new(self.events.subscribe())
    }
}

async fn drive(
    root: PathBuf,
    _watcher: Watch,
    mut source: broadcast::Receiver<WatchEvent>,
    current: tokio::sync::watch::Sender<Snapshot>,
    events: broadcast::Sender<watch::Event>,
) {
    let mut state: Snapshot = current.borrow().clone();
    loop {
        let received = tokio::select! {
            received = source.recv() => received,
            // The `DotAgents` is gone; nobody wants the changes.
            () = current.closed() => return,
        };
        match received {
            // Reloading reads the tree, and the backend reports those opens as
            // access events; skipping them stops the loop re-triggering itself.
            Ok(Ok(event)) if !relevant(&event) => continue,
            Ok(_) => {}
            // A lagged or failed watcher still means something changed.
            Err(RecvError::Lagged(_)) => {}
            Err(RecvError::Closed) => return,
        }

        let next = observe(&root).await;
        let changes = watch::diff(&state, &next);
        state = next.clone();
        // The state moves before the events that describe it, so a subscriber
        // reacting to one reads a state that already includes it.
        if current.send(next).is_err() {
            return;
        }
        for change in changes {
            // A change nobody is subscribed to is not a failure.
            let _ = events.send(change);
        }
    }
}

fn relevant(event: &notify::Event) -> bool {
    !matches!(event.kind, EventKind::Access(_))
}

async fn observe(root: &Path) -> Snapshot {
    match tokio::fs::metadata(root).await {
        Ok(meta) if meta.is_dir() => load(root).await.map(Arc::new),
        // Anything that is not a directory is no directory for this purpose.
        Ok(_) => Err(LoadError::Absent),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Err(LoadError::Absent),
        Err(error) => Err(unavailable(error)),
    }
}

async fn load(root: &Path) -> Result<Resources, LoadError> {
    let canonical = tokio::fs::canonicalize(root).await.map_err(unavailable)?;

    let (skills, mut diagnostics) = plugin::load_skills(root, &canonical).await;
    let (outcome, mcp_diagnostics) = plugin::load_mcp(root, &canonical, SpecVersion::V1_0_0).await;
    diagnostics.extend(mcp_diagnostics);
    let mcps = match outcome {
        plugin::McpOutcome::Absent => Vec::new(),
        plugin::McpOutcome::Configured(config) => config.servers,
        plugin::McpOutcome::Disabled(reason) => return Err(LoadError::Mcp(reason)),
    };
    let plugins = load_plugins(root).await?;

    Ok(Resources { plugins, skills, mcps, diagnostics })
}

async fn load_plugins(root: &Path) -> Result<Vec<plugin::Plugin>, LoadError> {
    let directory = root.join(PLUGINS_DIR);
    let mut entries = match tokio::fs::read_dir(&directory).await {
        Ok(entries) => entries,
        // A directory that ships no plugins is not a failure: it simply has none.
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(unavailable(error)),
    };

    let mut plugins = Vec::new();
    while let Some(entry) = entries.next_entry().await.map_err(unavailable)? {
        // Only a directory is a plugin; anything else is not one.
        if !entry.file_type().await.map_err(unavailable)?.is_dir() {
            continue;
        }
        // The plugin's id is its directory name, so a directory that has none
        // in text is not a plugin at all.
        if entry.file_name().to_str().is_none() {
            continue;
        }

        let plugin = plugin::load(entry.path()).await.map_err(|rejection| {
            LoadError::PluginRejected { directory: entry.path(), rejection }
        })?;
        if let plugin::McpOutcome::Disabled(reason) = &plugin.mcp {
            return Err(LoadError::PluginMcpDisabled {
                id: plugin.id.clone(),
                reason: reason.clone(),
            });
        }
        plugins.push(plugin);
    }
    plugins.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(plugins)
}

fn unavailable(error: io::Error) -> LoadError {
    LoadError::Unavailable { detail: error.to_string() }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    use super::*;
    use crate::mcp::MCP_SCHEMA_1_0_0;
    use crate::plugin::PLUGIN_SCHEMA_1_0_0;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let id = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir()
                .join(format!("dotagents-agents-{}-{id}", std::process::id()));
            std::fs::create_dir_all(&dir).expect("create temp dir");
            Self(std::fs::canonicalize(dir).expect("canonicalize temp dir"))
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().expect("a path to write to has a parent"))
            .expect("create dirs");
        std::fs::write(path, text).expect("write file");
    }

    fn write_manifest(root: &Path, name: &str) {
        let text = format!(r#"{{"$schema":"{PLUGIN_SCHEMA_1_0_0}","name":"{name}"}}"#);
        write(&root.join("plugin.json"), &text);
    }

    fn write_skill(root: &Path, id: &str, description: &str) {
        let text = format!("---\nname: {id}\ndescription: {description}\n---\n\n# Body\n");
        write(&root.join("skills").join(id).join("SKILL.md"), &text);
    }

    fn write_mcp(root: &Path, servers: &[(&str, &str)]) {
        let entries: Vec<String> = servers
            .iter()
            .map(|(name, url)| format!(r#""{name}":{{"type":"streamable-http","url":"{url}"}}"#))
            .collect();
        let text = format!(
            r#"{{"$schema":"{MCP_SCHEMA_1_0_0}","mcpServers":{{{}}}}}"#,
            entries.join(",")
        );
        write(&root.join("mcp.json"), &text);
    }

    fn loaded(state: &Snapshot) -> &Resources {
        match state {
            Ok(resources) => resources,
            Err(error) => panic!("expected resources, got {error}"),
        }
    }

    async fn wait_for(
        events: &mut watch::Subscription,
        mut wanted: impl FnMut(&watch::Event) -> bool,
    ) -> watch::Event {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let event = events.recv().await.expect("the watch stayed open");
                if wanted(&event) {
                    return event;
                }
            }
        })
        .await
        .expect("an event arrived before the timeout")
    }

    #[tokio::test]
    async fn loads_plugins_beside_the_directories_own_resources() {
        let tmp = TempDir::new();
        let root = tmp.path();
        write_manifest(&root.join(PLUGINS_DIR).join("demo"), "demo");
        write_skill(root, "greet", "Say hello.");
        write_mcp(root, &[("alpha", "https://a.example/mcp")]);

        let agents = DotAgents::open(root).await.expect("watch the directory");
        let state = agents.state();
        let resources = loaded(&state);
        assert_eq!(resources.plugins.len(), 1);
        assert_eq!(resources.plugins[0].id, "demo");
        assert_eq!(resources.skills.len(), 1);
        assert_eq!(resources.skills[0].directory, "greet");
        assert_eq!(resources.mcps.len(), 1);
        assert_eq!(resources.mcps[0].name, "alpha");
    }

    #[tokio::test]
    async fn a_plugins_components_stay_with_the_plugin() {
        let tmp = TempDir::new();
        let root = tmp.path();
        let plugin = root.join(PLUGINS_DIR).join("demo");
        write_manifest(&plugin, "demo");
        write_skill(&plugin, "inner", "Inside the plugin.");

        let agents = DotAgents::open(root).await.expect("watch the directory");
        let state = agents.state();
        let resources = loaded(&state);
        assert!(resources.skills.is_empty());
        assert_eq!(resources.plugins[0].skills.len(), 1);
        assert_eq!(resources.plugins[0].skills[0].directory, "inner");
    }

    #[tokio::test]
    async fn an_empty_directory_loads_nothing() {
        let tmp = TempDir::new();
        let agents = DotAgents::open(tmp.path()).await.expect("watch the directory");
        let state = agents.state();
        let resources = loaded(&state);
        assert!(resources.plugins.is_empty());
        assert!(resources.skills.is_empty());
        assert!(resources.mcps.is_empty());
        assert!(resources.diagnostics.is_empty());
    }

    #[tokio::test]
    async fn a_missing_directory_is_absent() {
        let tmp = TempDir::new();
        let missing = tmp.path().join("missing");
        let agents = DotAgents::open(&missing).await.expect("watch the directory");
        assert!(matches!(agents.state(), Err(LoadError::Absent)));
    }

    #[tokio::test]
    async fn a_rejected_plugin_makes_the_directory_invalid() {
        let tmp = TempDir::new();
        // No `plugin.json`: the directory is not a plugin.
        std::fs::create_dir_all(tmp.path().join(PLUGINS_DIR).join("broken")).expect("create dir");

        let agents = DotAgents::open(tmp.path()).await.expect("watch the directory");
        match agents.state() {
            Err(LoadError::PluginRejected { directory, .. }) => {
                assert!(directory.ends_with("plugins/broken"), "unexpected: {directory:?}")
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_disabled_plugin_mcp_makes_the_directory_invalid() {
        let tmp = TempDir::new();
        let plugin = tmp.path().join(PLUGINS_DIR).join("demo");
        write_manifest(&plugin, "demo");
        write(&plugin.join("mcp.json"), "not json");

        let agents = DotAgents::open(tmp.path()).await.expect("watch the directory");
        match agents.state() {
            Err(LoadError::PluginMcpDisabled { id, .. }) => assert_eq!(id, "demo"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_invalid_scope_mcp_makes_the_directory_invalid() {
        let tmp = TempDir::new();
        write(&tmp.path().join("mcp.json"), "not json");

        let agents = DotAgents::open(tmp.path()).await.expect("watch the directory");
        match agents.state() {
            Err(LoadError::Mcp(mcp::DisabledReason::NotJson { .. })) => {}
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_invalid_skill_is_skipped_and_reported() {
        let tmp = TempDir::new();
        write(&tmp.path().join("skills").join("greet").join("SKILL.md"), "not frontmatter");

        let agents = DotAgents::open(tmp.path()).await.expect("watch the directory");
        let state = agents.state();
        let resources = loaded(&state);
        assert!(resources.skills.is_empty());
        assert!(!resources.diagnostics.is_empty());
    }

    #[tokio::test]
    async fn a_changed_skill_reaches_a_subscriber_and_the_state() {
        let tmp = TempDir::new();
        let root = tmp.path();
        write_skill(root, "greet", "first");

        let agents = DotAgents::open(root).await.expect("watch the directory");
        let mut events = agents.subscribe();
        write_skill(root, "greet", "second");

        let event = wait_for(&mut events, |event| {
            matches!(event, watch::Event::Skill { change: watch::SkillChange::Modified(_), .. })
        })
        .await;
        match event {
            watch::Event::Skill { id, change: watch::SkillChange::Modified(skill) } => {
                assert_eq!(id, "greet");
                assert_eq!(skill.meta.description, "second");
            }
            other => panic!("unexpected: {other:?}"),
        }

        // The state moves before the event that describes it.
        let state = agents.state();
        assert_eq!(loaded(&state).skills[0].meta.description, "second");
    }

    #[tokio::test]
    async fn an_added_plugin_reaches_a_subscriber() {
        let tmp = TempDir::new();
        let root = tmp.path();
        write_manifest(root, "demo");

        let agents = DotAgents::open(root).await.expect("watch the directory");
        let mut events = agents.subscribe();
        write_manifest(&root.join(PLUGINS_DIR).join("extra"), "extra");

        let event = wait_for(&mut events, |event| {
            matches!(event, watch::Event::Plugin { change: watch::PluginChange::Added(_), .. })
        })
        .await;
        match event {
            watch::Event::Plugin { id, change: watch::PluginChange::Added(plugin) } => {
                assert_eq!(id, "extra");
                assert_eq!(plugin.manifest.name.as_str(), "extra");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_removed_skill_reaches_a_subscriber() {
        let tmp = TempDir::new();
        let root = tmp.path();
        write_skill(root, "greet", "Say hello.");

        let agents = DotAgents::open(root).await.expect("watch the directory");
        let mut events = agents.subscribe();
        std::fs::remove_dir_all(root.join("skills/greet")).expect("remove skill dir");

        let event = wait_for(&mut events, |event| {
            matches!(event, watch::Event::Skill { change: watch::SkillChange::Removed, .. })
        })
        .await;
        match event {
            watch::Event::Skill { id, .. } => assert_eq!(id, "greet"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test]
    async fn every_subscriber_gets_the_change() {
        let tmp = TempDir::new();
        let root = tmp.path();
        write_skill(root, "greet", "first");

        let agents = DotAgents::open(root).await.expect("watch the directory");
        let mut first = agents.subscribe();
        let mut second = agents.subscribe();
        write_skill(root, "greet", "second");

        for events in [&mut first, &mut second] {
            wait_for(events, |event| matches!(event, watch::Event::Skill { .. })).await;
        }
    }

    #[tokio::test]
    async fn a_directory_appearing_reports_its_resources() {
        let tmp = TempDir::new();
        let root = tmp.path().join(".agents");

        let agents = DotAgents::open(&root).await.expect("watch the directory");
        assert!(matches!(agents.state(), Err(LoadError::Absent)));
        let mut events = agents.subscribe();

        write_skill(&root, "greet", "Say hello.");

        let event = wait_for(&mut events, |event| {
            matches!(event, watch::Event::Skill { change: watch::SkillChange::Added(_), .. })
        })
        .await;
        match event {
            watch::Event::Skill { id, .. } => assert_eq!(id, "greet"),
            other => panic!("unexpected: {other:?}"),
        }
        let state = agents.state();
        assert_eq!(loaded(&state).skills[0].directory, "greet");
    }

    #[tokio::test]
    async fn a_broken_directory_is_reported_as_invalid() {
        let tmp = TempDir::new();
        let root = tmp.path();
        write_skill(root, "greet", "Say hello.");

        let agents = DotAgents::open(root).await.expect("watch the directory");
        let mut events = agents.subscribe();
        write(&root.join("mcp.json"), "not json");

        let event = wait_for(&mut events, |event| matches!(event, watch::Event::Invalid(_))).await;
        match event {
            watch::Event::Invalid(LoadError::Mcp(_)) => {}
            other => panic!("unexpected: {other:?}"),
        }
        assert!(matches!(agents.state(), Err(LoadError::Mcp(_))));
    }

    #[tokio::test]
    async fn dropping_the_directory_stops_the_subscription() {
        let tmp = TempDir::new();
        let agents = DotAgents::open(tmp.path()).await.expect("watch the directory");
        let mut events = agents.subscribe();

        drop(agents);

        let outcome = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("the watch stopped with the `DotAgents`");
        assert_eq!(outcome, Err(watch::SubscriptionError::Stopped));
    }
}
