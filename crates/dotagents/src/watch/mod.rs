//! Watch a `.agents/` package and report what changed in its resource terms.
//!
//! The watcher degrades the debouncer's raw filesystem events to a single
//! "something happened" signal, reloads the package, and reports the difference
//! against the previous state as [`Event`]s. Deriving changes from content
//! rather than from event kinds is what makes renames, invalid content, and
//! atomic saves all behave the same.
//!
//! The baseline is established silently: a consumer keeps its own initial load
//! and the watcher only reports increments.

mod event;
mod snapshot;

use std::path::{Path, PathBuf};
use std::time::Duration;

use notify::{EventKind, RecursiveMode};
use tokio::sync::mpsc;

use sandbox_toolkit_utils::watch::AsyncDebouncer;

use snapshot::{Snapshot, diff, observe};

pub use event::{Event, McpChange, PluginChange, SkillChange};
pub use sandbox_toolkit_utils::watch::Error;

const DEBOUNCE_TIMEOUT: Duration = Duration::from_millis(500);

const EVENT_CAPACITY: usize = 256;

/// A running watch over one `.agents/` package.
///
/// Dropping the watcher stops it on the next batch.
pub struct Watcher {
    events: mpsc::Receiver<Event>,
}

impl Watcher {
    /// Watch `root` using the default debounce interval.
    pub async fn new(root: impl AsRef<Path>) -> Result<Self, Error> {
        Self::with_debounce(root, DEBOUNCE_TIMEOUT).await
    }

    /// Watch `root`, waiting `timeout` of quiet before each reload.
    pub async fn with_debounce(root: impl AsRef<Path>, timeout: Duration) -> Result<Self, Error> {
        let root = root.as_ref().to_path_buf();
        let debouncer = AsyncDebouncer::new(timeout, None).await?;
        debouncer.watch(&root, RecursiveMode::Recursive).await?;

        // Establish the baseline after the watch is live, so no change slips
        // between the two; the reload's own read events are filtered below.
        let baseline = observe(&root).await;

        let (events, receiver) = mpsc::channel(EVENT_CAPACITY);
        tokio::spawn(drive(root, debouncer, baseline, events));
        Ok(Self { events: receiver })
    }

    /// Await the next change, or `None` once the watch has stopped.
    pub async fn recv(&mut self) -> Option<Event> {
        self.events.recv().await
    }

    /// Take the next change without waiting.
    pub fn try_recv(&mut self) -> Result<Event, mpsc::error::TryRecvError> {
        self.events.try_recv()
    }
}

async fn drive(
    root: PathBuf,
    mut debouncer: AsyncDebouncer,
    mut snapshot: Snapshot,
    events: mpsc::Sender<Event>,
) {
    loop {
        let batch = tokio::select! {
            batch = debouncer.recv() => batch,
            // The receiver is gone; nobody wants the batch.
            () = events.closed() => return,
        };
        let Some(batch) = batch else { return };
        // Reloading reads the files, and the backend reports those opens as
        // access events; skipping them is what stops the watcher from
        // re-triggering itself.
        let relevant = match &*batch {
            Ok(events) => events
                .iter()
                .any(|debounced| !matches!(debounced.event.kind, EventKind::Access(_))),
            Err(_) => false,
        };
        if !relevant {
            continue;
        }

        let next = observe(&root).await;
        let changes = diff(&snapshot, &next);
        snapshot = next;
        for change in changes {
            if events.send(change).await.is_err() {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use crate::mcp::MCP_SCHEMA_1_0_0;
    use crate::name::PluginName;
    use crate::plugin::PLUGIN_SCHEMA_1_0_0;

    const DEBOUNCE: Duration = Duration::from_millis(150);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let id = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("dotagents-watch-{}-{id}", std::process::id()));
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

    fn write_manifest(root: &Path, name: &str) {
        let text = format!(r#"{{"$schema":"{PLUGIN_SCHEMA_1_0_0}","name":"{name}"}}"#);
        std::fs::write(root.join("plugin.json"), text).expect("write plugin.json");
    }

    fn write_skill(root: &Path, id: &str, description: &str) {
        let dir = root.join("skills").join(id);
        std::fs::create_dir_all(&dir).expect("create skill dir");
        let text = format!("---\nname: {id}\ndescription: {description}\n---\n\n# Body\n");
        std::fs::write(dir.join("SKILL.md"), text).expect("write SKILL.md");
    }

    fn write_mcp(root: &Path, servers: &[(&str, &str)]) {
        let entries: Vec<String> = servers
            .iter()
            .map(|(name, url)| {
                format!(r#""{name}":{{"type":"streamable-http","url":"{url}"}}"#)
            })
            .collect();
        let text = format!(
            r#"{{"$schema":"{MCP_SCHEMA_1_0_0}","mcpServers":{{{}}}}}"#,
            entries.join(",")
        );
        std::fs::write(root.join("mcp.json"), text).expect("write mcp.json");
    }

    async fn watcher(root: &Path) -> Watcher {
        Watcher::with_debounce(root, DEBOUNCE).await.expect("start watcher")
    }

    async fn wait_for(watcher: &mut Watcher, mut wanted: impl FnMut(&Event) -> bool) -> Event {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let event = watcher.recv().await.expect("watcher stayed open");
                if wanted(&event) {
                    return event;
                }
            }
        })
        .await
        .expect("an event arrived before the timeout")
    }

    #[tokio::test]
    async fn the_baseline_is_silent() {
        let tmp = TempDir::new();
        let root = tmp.path();
        write_manifest(root, "demo");
        write_skill(root, "greet", "first");

        let mut watcher = watcher(root).await;
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert!(matches!(watcher.try_recv(), Err(mpsc::error::TryRecvError::Empty)));
    }

    #[tokio::test]
    async fn modifying_skill_md_reports_a_modification() {
        let tmp = TempDir::new();
        let root = tmp.path();
        write_manifest(root, "demo");
        write_skill(root, "greet", "first");

        let mut watcher = watcher(root).await;
        write_skill(root, "greet", "second");

        let event = wait_for(&mut watcher, |event| {
            matches!(event, Event::Skill { change: SkillChange::Modified(_), .. })
        })
        .await;
        match event {
            Event::Skill { id, change: SkillChange::Modified(skill), .. } => {
                assert_eq!(id, "greet");
                assert_eq!(skill.meta.description, "second");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn removing_a_skill_reports_a_removal() {
        let tmp = TempDir::new();
        let root = tmp.path();
        write_manifest(root, "demo");
        write_skill(root, "greet", "first");

        let mut watcher = watcher(root).await;
        std::fs::remove_dir_all(root.join("skills/greet")).expect("remove skill dir");

        let event = wait_for(&mut watcher, |event| {
            matches!(event, Event::Skill { change: SkillChange::Removed, .. })
        })
        .await;
        match event {
            Event::Skill { id, .. } => assert_eq!(id, "greet"),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_invalid_skill_is_not_a_removal() {
        let tmp = TempDir::new();
        let root = tmp.path();
        write_manifest(root, "demo");
        write_skill(root, "greet", "first");

        let mut watcher = watcher(root).await;
        std::fs::write(root.join("skills/greet/SKILL.md"), "not frontmatter").expect("break skill");

        let event = wait_for(&mut watcher, |event| {
            matches!(event, Event::Skill { change: SkillChange::Invalid(_), .. })
        })
        .await;
        match event {
            Event::Skill { change: SkillChange::Invalid(diagnostics), .. } => {
                assert!(!diagnostics.is_empty());
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn reloading_mcp_reports_the_server_difference() {
        let tmp = TempDir::new();
        let root = tmp.path();
        write_manifest(root, "demo");
        write_mcp(root, &[("alpha", "https://a.example/mcp")]);

        let mut watcher = watcher(root).await;
        write_mcp(
            root,
            &[("alpha", "https://a.example/mcp"), ("beta", "https://b.example/mcp")],
        );

        let event = wait_for(&mut watcher, |event| matches!(event, Event::Mcp { .. })).await;
        match event {
            Event::Mcp { change: McpChange::Reloaded { added, removed }, .. } => {
                let added: Vec<&str> = added.iter().map(|entry| entry.name.as_str()).collect();
                assert_eq!(added, ["beta"]);
                assert!(removed.is_empty());
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_invalid_mcp_is_reported_as_such() {
        let tmp = TempDir::new();
        let root = tmp.path();
        write_manifest(root, "demo");
        write_mcp(root, &[("alpha", "https://a.example/mcp")]);

        let mut watcher = watcher(root).await;
        std::fs::write(root.join("mcp.json"), "not json").expect("break mcp.json");

        let event = wait_for(&mut watcher, |event| {
            matches!(event, Event::Mcp { change: McpChange::Invalid(_), .. })
        })
        .await;
        match event {
            Event::Mcp { change: McpChange::Invalid(diagnostics), .. } => {
                assert!(!diagnostics.is_empty());
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn renaming_the_plugin_is_a_removal_then_an_addition() {
        let tmp = TempDir::new();
        let root = tmp.path();
        write_manifest(root, "alpha");

        let mut watcher = watcher(root).await;
        write_manifest(root, "beta");

        let removed = wait_for(&mut watcher, |event| {
            matches!(event, Event::Plugin { change: PluginChange::Removed { .. }, .. })
        })
        .await;
        let added = wait_for(&mut watcher, |event| {
            matches!(event, Event::Plugin { change: PluginChange::Added(_), .. })
        })
        .await;

        match removed {
            Event::Plugin { plugin, .. } => assert_eq!(plugin.as_ref().map(PluginName::as_str), Some("alpha")),
            other => panic!("unexpected event: {other:?}"),
        }
        match added {
            Event::Plugin { plugin, .. } => assert_eq!(plugin.as_ref().map(PluginName::as_str), Some("beta")),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_fatal_manifest_is_reported_as_invalid() {
        let tmp = TempDir::new();
        let root = tmp.path();
        write_manifest(root, "demo");
        write_skill(root, "greet", "first");

        let mut watcher = watcher(root).await;
        std::fs::write(root.join("plugin.json"), "not json").expect("break manifest");

        let event = wait_for(&mut watcher, |event| {
            matches!(event, Event::Plugin { change: PluginChange::Invalid { .. }, .. })
        })
        .await;
        match event {
            Event::Plugin { plugin, change: PluginChange::Invalid { skills, mcp, .. } } => {
                assert_eq!(plugin.as_ref().map(PluginName::as_str), Some("demo"));
                assert!(skills, "the package still held a skill");
                assert!(!mcp);
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn editing_extension_data_is_a_modified_manifest() {
        let tmp = TempDir::new();
        let root = tmp.path();
        let manifest = |flag: &str| {
            format!(
                r#"{{"$schema":"{PLUGIN_SCHEMA_1_0_0}","name":"demo","extensions":{{"com.example.client":{{"on":{flag}}}}}}}"#
            )
        };
        std::fs::write(root.join("plugin.json"), manifest("true")).expect("write plugin.json");

        let mut watcher = watcher(root).await;
        std::fs::write(root.join("plugin.json"), manifest("false")).expect("edit plugin.json");

        let event = wait_for(&mut watcher, |event| {
            matches!(event, Event::Plugin { change: PluginChange::Modified(_), .. })
        })
        .await;
        match event {
            Event::Plugin { change: PluginChange::Modified(manifest), .. } => {
                assert!(manifest.extensions.raw("com.example.client").is_some());
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn removing_the_manifest_carries_the_components_it_held() {
        let tmp = TempDir::new();
        let root = tmp.path();
        write_manifest(root, "demo");
        write_skill(root, "greet", "first");

        let mut watcher = watcher(root).await;
        std::fs::remove_file(root.join("plugin.json")).expect("remove plugin.json");

        let event = wait_for(&mut watcher, |event| {
            matches!(event, Event::Plugin { change: PluginChange::Removed { .. }, .. })
        })
        .await;
        match event {
            Event::Plugin { change: PluginChange::Removed { skills, mcp }, .. } => {
                assert!(skills, "the package still held a skill");
                assert!(!mcp, "the package held no mcp.json");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }
}
