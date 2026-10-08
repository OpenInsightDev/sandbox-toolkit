mod debounce;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use notify::{Event, RecursiveMode};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

pub use debounce::{AsyncDebouncer, Error};

const DEBOUNCE_TIMEOUT: Duration = Duration::from_millis(500);

const BROADCAST_CAPACITY: usize = 256;

/// A watcher error delivered on the event stream.
///
/// [`Error`] is not `Clone`, so it is shared behind an [`Arc`] to travel over
/// the broadcast channel.
pub type WatchError = Arc<Error>;

/// An item produced by a [`Watch`]: either an event or a watcher error.
pub type WatchEvent = Result<Event, WatchError>;

/// What the debouncer is currently attached to.
///
/// A path that exists as a directory is watched directly; a file or a path that
/// does not exist yet is watched through its parent.
#[derive(Clone, Copy)]
enum Attachment {
    Target,
    Parent,
}

#[derive(Debug)]
pub struct Watch {
    events: broadcast::Sender<WatchEvent>,
    handle: JoinHandle<()>,
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

impl Watch {
    pub async fn new(path: impl Into<PathBuf>) -> Result<Self, Error> {
        Self::start(path.into(), RecursiveMode::NonRecursive).await
    }

    pub async fn recursive(path: impl Into<PathBuf>) -> Result<Self, Error> {
        Self::start(path.into(), RecursiveMode::Recursive).await
    }

    pub fn subscribe(&self) -> broadcast::Receiver<WatchEvent> {
        self.events.subscribe()
    }

    async fn start(path: PathBuf, mode: RecursiveMode) -> Result<Self, Error> {
        let parent = match path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
            _ => PathBuf::from("."),
        };

        let mut debouncer = AsyncDebouncer::new(DEBOUNCE_TIMEOUT, None).await?;

        // A file is watched through its parent: an inode watch is lost when the
        // file is replaced, while the parent keeps reporting the file's own
        // events. Sibling events are filtered out by `confine`.
        let mut attachment = Attachment::Target;
        if path.is_dir() {
            debouncer.watch(&path, mode).await?;
        } else {
            attachment = Attachment::Parent;
            debouncer
                .watch(&parent, RecursiveMode::NonRecursive)
                .await?;
        }

        let (events, _) = broadcast::channel(BROADCAST_CAPACITY);

        let task_events = events.clone();
        let handle = tokio::spawn(async move {
            loop {
                let Some(result) = debouncer.recv().await else {
                    break;
                };

                let outcome: Result<(), Error> = async {
                    match Arc::try_unwrap(result) {
                        Ok(Ok(batch)) => {
                            for debounced in batch {
                                let Some(event) = confine(&debounced.event, &path, attachment)
                                else {
                                    continue;
                                };
                                let _ = task_events.send(Ok(event));
                            }
                        }
                        Ok(Err(errors)) => {
                            for error in errors {
                                let _ = task_events.send(Err(Arc::new(Error::from(error))));
                            }
                            return Ok(());
                        }
                        // The batch is always uniquely owned; a stray clone is
                        // dropped because its non-`Clone` errors cannot be
                        // forwarded.
                        Err(_) => return Ok(()),
                    }

                    match (attachment, path.is_dir()) {
                        (Attachment::Parent, true) => {
                            debouncer.unwatch(&parent).await?;
                            debouncer.watch(&path, mode).await?;
                            attachment = Attachment::Target;
                        }
                        (Attachment::Target, false) => {
                            let _ = debouncer.unwatch(&path).await;
                            debouncer
                                .watch(&parent, RecursiveMode::NonRecursive)
                                .await?;
                            attachment = Attachment::Parent;
                        }
                        _ => {}
                    }

                    Ok(())
                }
                .await;

                if let Err(error) = outcome {
                    // The watcher cannot recover from a failed re-attach, so the
                    // error is the last item on the stream.
                    let _ = task_events.send(Err(Arc::new(error)));
                    break;
                }
            }
        });

        Ok(Self { events, handle })
    }
}

/// Narrows an event to the watched target.
///
/// Attached to a directory, every reported path is already inside the target.
/// Attached to a parent, the backend also reports sibling changes, so only the
/// paths equal to the target are kept; an event without one is dropped.
fn confine(event: &Event, path: &Path, attachment: Attachment) -> Option<Event> {
    if let Attachment::Target = attachment {
        return Some(event.clone());
    }

    let mut confined = event.clone();
    confined
        .paths
        .retain(|candidate| candidate.as_path() == path);
    (!confined.paths.is_empty()).then_some(confined)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    fn temp_dir() -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("sandbox-toolkit-watch-{id}"));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        std::fs::canonicalize(dir).expect("canonicalize temp dir")
    }

    #[tokio::test]
    async fn a_file_target_reports_its_own_changes_and_ignores_siblings() {
        let dir = temp_dir();
        let target = dir.join("watched.txt");
        std::fs::write(&target, "0").expect("seed target");

        let watch = Watch::new(&target).await.expect("start watching");
        let mut events = watch.subscribe();
        // Let the background task reach its receive loop before changing anything.
        tokio::time::sleep(Duration::from_millis(500)).await;

        std::fs::write(dir.join("sibling.txt"), "noise").expect("write sibling");
        std::fs::write(&target, "1").expect("write target");

        let first = tokio::time::timeout(Duration::from_secs(10), events.recv())
            .await
            .expect("the target change is reported")
            .expect("the watch stays open")
            .expect("the watcher does not fail");
        assert_eq!(first.paths, vec![target.clone()]);

        while let Ok(Ok(Ok(event))) =
            tokio::time::timeout(Duration::from_millis(900), events.recv()).await
        {
            assert!(
                event.paths.iter().all(|path| path == &target),
                "only the target is reported, got {:?}",
                event.paths
            );
        }

        std::fs::remove_dir_all(&dir).expect("clean temp dir");
    }
}
