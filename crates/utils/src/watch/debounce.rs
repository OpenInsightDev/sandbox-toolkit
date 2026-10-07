use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use notify::{Config, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, new_debouncer};
use tokio::sync::{mpsc, oneshot};

/// Errors returned by the async watcher.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The underlying debouncer failed to be created or controlled.
    #[error(transparent)]
    Notify(#[from] notify::Error),

    /// The driver thread could not be started.
    #[error("failed to spawn the watcher driver thread: {0}")]
    Spawn(#[source] std::io::Error),

    /// The driver thread is gone, leaving nothing to control.
    #[error("the watcher driver is no longer running")]
    DriverGone,
}

/// A debounced batch carried over the channel.
///
/// Wrapped in [`Arc`] because [`DebounceEventResult`] is not `Clone`.
pub type EventBatch = Arc<DebounceEventResult>;

/// A control request handled by the driver thread.
///
/// Each request carries the channel its result travels back on, so the caller
/// resumes only once the driver has applied it.
enum Command {
    Watch {
        path: PathBuf,
        mode: RecursiveMode,
        reply: oneshot::Sender<notify::Result<()>>,
    },
    Unwatch {
        path: PathBuf,
        reply: oneshot::Sender<notify::Result<()>>,
    },
    Configure {
        option: Config,
        reply: oneshot::Sender<notify::Result<bool>>,
    },
    Stop {
        reply: oneshot::Sender<()>,
    },
}

/// An async handle to a `notify-debouncer-full` debouncer.
///
/// A dedicated thread owns the debouncer and applies control commands in order;
/// batches of events are forwarded to the single consumer over a channel. Sole
/// ownership is what keeps the debouncer lock-free and lets
/// [`AsyncDebouncer::stop`] always reach it.
///
/// Dropping the value stops the debouncer without waiting. Use
/// [`AsyncDebouncer::stop`] to wait for its event thread to finish.
pub struct AsyncDebouncer {
    commands: mpsc::UnboundedSender<Command>,
    events: mpsc::UnboundedReceiver<EventBatch>,
}

impl AsyncDebouncer {
    /// Create a debouncer.
    ///
    /// `timeout` is the amount of time after which a debounced event is
    /// emitted. If `tick_rate` is `None`, notify selects one that is 1/4 of
    /// `timeout`.
    pub async fn new(timeout: Duration, tick_rate: Option<Duration>) -> Result<Self, Error> {
        let (commands, command_rx) = mpsc::unbounded_channel();
        let (event_tx, events) = mpsc::unbounded_channel();
        let (ready_tx, ready) = oneshot::channel();

        std::thread::Builder::new()
            .name("watch-debouncer".to_string())
            .spawn(move || drive(timeout, tick_rate, command_rx, event_tx, ready_tx))
            .map_err(Error::Spawn)?;

        // Creating the debouncer touches the filesystem, so it happens on the
        // driver thread and any failure is reported back instead of panicking it.
        ready.await.map_err(|_| Error::DriverGone)??;

        Ok(Self { commands, events })
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
        self.request(|reply| Command::Watch {
            path,
            mode: recursive_mode,
            reply,
        })
        .await
    }

    /// Stop watching `path`.
    pub async fn unwatch(&self, path: impl AsRef<Path>) -> Result<(), Error> {
        let path = path.as_ref().to_path_buf();
        self.request(|reply| Command::Unwatch { path, reply }).await
    }

    /// Apply a watcher configuration option.
    ///
    /// Returns `true` if the option was applied, `false` if the backend does
    /// not support it.
    pub async fn configure(&self, option: Config) -> Result<bool, Error> {
        self.request(|reply| Command::Configure { option, reply })
            .await
    }

    /// Stop the debouncer and wait for its event thread to finish.
    ///
    /// This waits for up to one `tick_rate`. Because the driver owns the
    /// debouncer, the stop is unconditional.
    pub async fn stop(self) -> Result<(), Error> {
        let (reply, stopped) = oneshot::channel();
        self.commands
            .send(Command::Stop { reply })
            .map_err(|_| Error::DriverGone)?;
        stopped.await.map_err(|_| Error::DriverGone)
    }

    async fn request<T>(
        &self,
        command: impl FnOnce(oneshot::Sender<notify::Result<T>>) -> Command,
    ) -> Result<T, Error> {
        let (reply, result) = oneshot::channel();
        self.commands
            .send(command(reply))
            .map_err(|_| Error::DriverGone)?;
        result
            .await
            .map_err(|_| Error::DriverGone)?
            .map_err(Error::from)
    }
}

/// Owns the debouncer and serves control commands until the handle is dropped.
fn drive(
    timeout: Duration,
    tick_rate: Option<Duration>,
    mut commands: mpsc::UnboundedReceiver<Command>,
    events: mpsc::UnboundedSender<EventBatch>,
    ready: oneshot::Sender<Result<(), Error>>,
) {
    let mut debouncer =
        match new_debouncer(timeout, tick_rate, move |result: DebounceEventResult| {
            // Synchronous, non-blocking send. The only failure mode is a closed
            // receiver, which is harmless: nobody wants the batch.
            let _ = events.send(Arc::new(result));
        }) {
            Ok(debouncer) => {
                let _ = ready.send(Ok(()));
                debouncer
            }
            Err(error) => {
                let _ = ready.send(Err(error.into()));
                return;
            }
        };

    while let Some(command) = commands.blocking_recv() {
        match command {
            Command::Watch { path, mode, reply } => {
                let _ = reply.send(debouncer.watch(path, mode));
            }
            Command::Unwatch { path, reply } => {
                let _ = reply.send(debouncer.unwatch(path));
            }
            Command::Configure { option, reply } => {
                let _ = reply.send(debouncer.configure(option));
            }
            Command::Stop { reply } => {
                debouncer.stop();
                let _ = reply.send(());
                return;
            }
        }
    }

    // The handle is gone, so drop the debouncer without waiting for its event
    // thread; `Drop` signals the stop.
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
