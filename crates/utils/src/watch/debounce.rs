use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use notify::{Config, RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};
use tokio::sync::mpsc;

/// Errors returned by the async watcher.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The underlying debouncer failed to be created or controlled.
    #[error(transparent)]
    Notify(#[from] notify::Error),

    /// The lock guarding the debouncer was poisoned by a panicking task.
    #[error("the debouncer lock was poisoned")]
    Poisoned,

    /// A [`tokio::task::spawn_blocking`] task panicked or was cancelled.
    #[error("a blocking watcher task failed: {0}")]
    Join(#[from] tokio::task::JoinError),
}

type Inner = Debouncer<RecommendedWatcher, RecommendedCache>;

/// A debounced batch carried over the channel.
///
/// Wrapped in [`Arc`] because [`DebounceEventResult`] is not `Clone`.
pub type EventBatch = Arc<DebounceEventResult>;

/// An async wrapper around a `notify-debouncer-full` debouncer.
///
/// The debouncer runs on its own thread; batches of events are forwarded to the
/// single consumer through an mpsc channel. Await the next batch with
/// [`AsyncDebouncer::recv`], or poll for one without waiting with
/// [`AsyncDebouncer::try_recv`].
///
/// Dropping the value stops the debouncer (non-blockingly). Use
/// [`AsyncDebouncer::stop`] when you want to wait for its thread to finish.
pub struct AsyncDebouncer {
    inner: Arc<Mutex<Inner>>,
    events: mpsc::UnboundedReceiver<EventBatch>,
}

impl AsyncDebouncer {
    /// Create a debouncer.
    ///
    /// `timeout` is the amount of time after which a debounced event is
    /// emitted. If `tick_rate` is `None`, notify selects one that is 1/4 of
    /// `timeout`.
    pub async fn new(timeout: Duration, tick_rate: Option<Duration>) -> Result<Self, Error> {
        let (tx, events) = mpsc::unbounded_channel::<EventBatch>();

        // Creating the watcher touches the filesystem / OS notification APIs,
        // so keep it off the async worker threads too.
        let inner = tokio::task::spawn_blocking(move || {
            new_debouncer(timeout, tick_rate, move |result: DebounceEventResult| {
                // Synchronous, non-blocking send. The only failure mode is a
                // closed receiver, which is harmless: nobody wants the batch.
                let _ = tx.send(Arc::new(result));
            })
        })
        .await??;

        Ok(Self {
            inner: Arc::new(Mutex::new(inner)),
            events,
        })
    }

    /// Await the next debounced batch.
    ///
    /// Returns `None` once the debouncer has been stopped or dropped.
    pub async fn recv(&mut self) -> Option<EventBatch> {
        self.events.recv().await
    }

    /// Try to receive a debounced batch without waiting.
    ///
    /// Returns [`mpsc::error::TryRecvError::Empty`] when no batch is ready yet,
    /// and [`mpsc::error::TryRecvError::Disconnected`] once the debouncer has
    /// been stopped or dropped.
    pub fn try_recv(&mut self) -> Result<EventBatch, mpsc::error::TryRecvError> {
        self.events.try_recv()
    }

    /// Start watching `path` for changes.
    pub async fn watch(
        &self,
        path: impl AsRef<Path>,
        recursive_mode: RecursiveMode,
    ) -> Result<(), Error> {
        let path = path.as_ref().to_path_buf();
        let inner = Arc::clone(&self.inner);

        tokio::task::spawn_blocking(move || {
            let mut debouncer = inner.lock().map_err(|_| Error::Poisoned)?;
            debouncer.watch(&path, recursive_mode).map_err(Error::from)
        })
        .await?
    }

    /// Stop watching `path`.
    pub async fn unwatch(&self, path: impl AsRef<Path>) -> Result<(), Error> {
        let path = path.as_ref().to_path_buf();
        let inner = Arc::clone(&self.inner);

        tokio::task::spawn_blocking(move || {
            let mut debouncer = inner.lock().map_err(|_| Error::Poisoned)?;
            debouncer.unwatch(&path).map_err(Error::from)
        })
        .await?
    }

    /// Apply a watcher configuration option.
    ///
    /// Returns `true` if the option was applied, `false` if the backend does
    /// not support it.
    pub async fn configure(&self, option: Config) -> Result<bool, Error> {
        let inner = Arc::clone(&self.inner);

        tokio::task::spawn_blocking(move || {
            let mut debouncer = inner.lock().map_err(|_| Error::Poisoned)?;
            debouncer.configure(option).map_err(Error::from)
        })
        .await?
    }

    /// Stop the debouncer and wait for its event thread to finish.
    ///
    /// This waits for up to one `tick_rate`, on a blocking thread so the async
    /// runtime stays responsive.
    pub async fn stop(self) -> Result<(), Error> {
        tokio::task::spawn_blocking(move || match Arc::try_unwrap(self.inner) {
            Ok(mutex) => {
                let debouncer = mutex.into_inner().map_err(|_| Error::Poisoned)?;
                debouncer.stop();
                Ok(())
            }
            // A control operation is still holding a clone of the handle.
            // Dropping ours lets the debouncer stop non-blockingly once that
            // operation releases it.
            Err(_) => Ok(()),
        })
        .await?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn create_watch_and_stop() {
        let watcher = AsyncDebouncer::new(Duration::from_millis(50), None)
            .await
            .expect("create debouncer");

        watcher
            .watch(std::env::temp_dir(), RecursiveMode::NonRecursive)
            .await
            .expect("watch temp dir");

        watcher.stop().await.expect("stop debouncer");
    }

    #[tokio::test]
    async fn no_batch_is_ready_before_any_change() {
        let mut watcher = AsyncDebouncer::new(Duration::from_millis(50), None)
            .await
            .expect("create debouncer");

        assert!(matches!(
            watcher.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));

        watcher.stop().await.expect("stop debouncer");
    }
}
