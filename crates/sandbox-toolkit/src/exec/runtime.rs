use std::ffi::OsString;
use std::io;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{ExitStatus, Stdio};

use bytes::Bytes;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_stream::Stream;
use tokio_stream::wrappers::ReceiverStream;

use crate::exec::frame::Status;

/// Output is read in chunks bounded by this size, so a read is always small
/// enough for one frame.
const READ_LEN: usize = 8 * 1024;

/// Frames buffered between the readers and the response body.
const BUFFER: usize = 16;

/// What a command is observed to do: its output, then how it ended.
pub enum Event {
    Stdout(Bytes),
    Stderr(Bytes),
    Status(Status),
}

/// A running command whose output arrives as [`Event`]s.
pub struct Execution {
    events: mpsc::Receiver<Event>,
}

impl Execution {
    pub fn spawn(
        program: &str,
        args: &[String],
        cwd: &Path,
        env: &[(OsString, OsString)],
    ) -> io::Result<Self> {
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(cwd)
            .env_clear()
            .envs(env.iter().cloned())
            // exec carries no input, so the child must not read the service's
            // own stdin.
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = command.spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("stdout is not piped"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| io::Error::other("stderr is not piped"))?;

        let (events, receiver) = mpsc::channel(BUFFER);
        let stdout = pump(stdout, Event::Stdout, events.clone());
        let stderr = pump(stderr, Event::Stderr, events.clone());

        tokio::spawn(async move {
            // Readers drain first: the child exits while output it wrote may
            // still be unread, and the status must not overtake it.
            let status = status(child.wait().await);
            let _ = tokio::join!(stdout, stderr);
            let _ = events.send(Event::Status(status)).await;
        });

        Ok(Self { events: receiver })
    }

    pub async fn recv(&mut self) -> Option<Event> {
        self.events.recv().await
    }

    /// The events still to come.
    pub fn into_stream(self) -> impl Stream<Item = Event> {
        ReceiverStream::new(self.events)
    }
}

/// Forwards everything one of the child's streams produces as the events it
/// makes.
fn pump<R>(mut reader: R, event: fn(Bytes) -> Event, events: mpsc::Sender<Event>) -> JoinHandle<()>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut buffer = vec![0; READ_LEN];
        loop {
            match reader.read(&mut buffer).await {
                Ok(0) => break,
                Ok(read) => {
                    let bytes = Bytes::copy_from_slice(&buffer[..read]);
                    if events.send(event(bytes)).await.is_err() {
                        break;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
    })
}

/// An unwaitable child has no status to report, so it exits as `-1`.
fn status(waited: io::Result<ExitStatus>) -> Status {
    let Ok(waited) = waited else {
        return Status::Exited { exit_code: -1 };
    };

    match waited.signal() {
        Some(signal) => Status::Signaled { signal },
        None => Status::Exited {
            exit_code: waited.code().unwrap_or(-1),
        },
    }
}
