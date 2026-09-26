use core::fmt;
use std::io;
use std::process::ExitStatus;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicBool;

use anyhow::anyhow;
use portable_pty::MasterPty;
use portable_pty::PtySize;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::sync::watch;
use tokio::task::AbortHandle;
use tokio::task::JoinHandle;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessSignal {
    Interrupt,
}

pub(crate) fn unsupported_signal(signal: ProcessSignal) -> io::Error {
    match signal {
        ProcessSignal::Interrupt => io::Error::new(
            io::ErrorKind::Unsupported,
            "process interrupt is not supported by this process backend",
        ),
    }
}

/// Terminal outcome of a spawned process.
///
/// A signal-terminated process keeps the backend's fallback exit code, so
/// callers distinguish a real exit from `SIGKILL`/`SIGTERM` by [`Self::signal`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessExit {
    pub exit_code: i32,
    pub signal: Option<String>,
}

impl ProcessExit {
    pub fn exited(exit_code: i32) -> Self {
        Self {
            exit_code,
            signal: None,
        }
    }

    pub fn signaled(exit_code: i32, signal: impl Into<String>) -> Self {
        Self {
            exit_code,
            signal: Some(signal.into()),
        }
    }
}

pub(crate) fn process_exit_from_status(status: ExitStatus) -> ProcessExit {
    use std::os::unix::process::ExitStatusExt;

    if let Some(signal) = status.signal() {
        return ProcessExit::signaled(128 + signal, signal_name(signal));
    }

    ProcessExit::exited(status.code().unwrap_or(-1))
}

fn signal_name(signal: i32) -> String {
    let name = unsafe { libc::strsignal(signal) };
    if name.is_null() {
        return format!("signal {signal}");
    }

    unsafe { std::ffi::CStr::from_ptr(name) }
        .to_string_lossy()
        .into_owned()
}

pub(crate) trait ChildTerminator: Send + Sync {
    fn signal(&mut self, signal: ProcessSignal) -> io::Result<()>;

    fn kill(&mut self) -> io::Result<()>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalSize {
    pub rows: u16,
    pub cols: u16,
}

impl Default for TerminalSize {
    fn default() -> Self {
        Self { rows: 24, cols: 80 }
    }
}

impl From<TerminalSize> for PtySize {
    fn from(value: TerminalSize) -> Self {
        Self {
            rows: value.rows,
            cols: value.cols,
            pixel_width: 0,
            pixel_height: 0,
        }
    }
}

/// Callback used by driver-backed sessions to resize a PTY-like backend when
/// there is no local master handle to resize directly.
type ResizeFn = Box<dyn FnMut(TerminalSize) -> anyhow::Result<()> + Send>;

/// Handle for driving an interactive process (PTY or pipe).
pub struct ProcessHandle {
    writer_tx: StdMutex<Option<mpsc::Sender<Vec<u8>>>>,
    killer: StdMutex<Option<Box<dyn ChildTerminator>>>,
    reader_handle: StdMutex<Option<JoinHandle<()>>>,
    reader_abort_handles: StdMutex<Vec<AbortHandle>>,
    writer_handle: StdMutex<Option<JoinHandle<()>>>,
    wait_handle: StdMutex<Option<JoinHandle<()>>>,
    exit_status: Arc<AtomicBool>,
    exit: Arc<StdMutex<Option<ProcessExit>>>,
    // The PTY master must be preserved: the child receives Control+C if it is
    // dropped while the child is still running.
    _pty_master: StdMutex<Option<Box<dyn MasterPty + Send>>>,
    // Optional resize hook for driver-backed sessions that proxy PTY control to
    // another backend instead of owning a local master.
    resizer: StdMutex<Option<ResizeFn>>,
}

impl fmt::Debug for ProcessHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProcessHandle").finish()
    }
}

impl ProcessHandle {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        writer_tx: mpsc::Sender<Vec<u8>>,
        killer: Box<dyn ChildTerminator>,
        reader_handle: JoinHandle<()>,
        reader_abort_handles: Vec<AbortHandle>,
        writer_handle: JoinHandle<()>,
        wait_handle: JoinHandle<()>,
        exit_status: Arc<AtomicBool>,
        exit: Arc<StdMutex<Option<ProcessExit>>>,
        pty_master: Option<Box<dyn MasterPty + Send>>,
        resizer: Option<ResizeFn>,
    ) -> Self {
        Self {
            writer_tx: StdMutex::new(Some(writer_tx)),
            killer: StdMutex::new(Some(killer)),
            reader_handle: StdMutex::new(Some(reader_handle)),
            reader_abort_handles: StdMutex::new(reader_abort_handles),
            writer_handle: StdMutex::new(Some(writer_handle)),
            wait_handle: StdMutex::new(Some(wait_handle)),
            exit_status,
            exit,
            _pty_master: StdMutex::new(pty_master),
            resizer: StdMutex::new(resizer),
        }
    }

    /// Returns a channel sender for writing raw bytes to the child stdin.
    pub fn writer_sender(&self) -> mpsc::Sender<Vec<u8>> {
        if let Ok(writer_tx) = self.writer_tx.lock()
            && let Some(writer_tx) = writer_tx.as_ref()
        {
            return writer_tx.clone();
        }

        let (writer_tx, writer_rx) = mpsc::channel(1);
        drop(writer_rx);
        writer_tx
    }

    /// True if the child process has exited.
    pub fn has_exited(&self) -> bool {
        self.exit_status.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Returns the terminal outcome if the child has exited.
    pub fn exit_status(&self) -> Option<ProcessExit> {
        self.exit.lock().ok().and_then(|guard| guard.clone())
    }

    /// Returns the exit code if known.
    pub fn exit_code(&self) -> Option<i32> {
        self.exit_status().map(|status| status.exit_code)
    }

    /// Returns the terminating signal if the child was signalled.
    pub fn exit_signal(&self) -> Option<String> {
        self.exit_status().and_then(|status| status.signal)
    }

    /// Resize the PTY in character cells.
    pub fn resize(&self, size: TerminalSize) -> anyhow::Result<()> {
        {
            let master = self
                ._pty_master
                .lock()
                .map_err(|_| anyhow!("failed to lock PTY master"))?;
            if let Some(master) = master.as_ref() {
                return master.resize(size.into());
            }
        }

        let mut resizer = self
            .resizer
            .lock()
            .map_err(|_| anyhow!("failed to lock PTY resizer"))?;
        if let Some(resizer) = resizer.as_mut() {
            resizer(size)
        } else {
            Err(anyhow!("process is not attached to a PTY"))
        }
    }

    /// Close the child's stdin channel.
    pub fn close_stdin(&self) {
        if let Ok(mut writer_tx) = self.writer_tx.lock() {
            writer_tx.take();
        }
    }

    /// Attempts to kill the child while leaving the reader/writer tasks alive
    /// so callers can still drain output until EOF.
    pub fn request_terminate(&self) {
        if let Ok(mut killer_opt) = self.killer.lock()
            && let Some(mut killer) = killer_opt.take()
        {
            let _ = killer.kill();
        }
    }

    pub fn signal(&self, signal: ProcessSignal) -> io::Result<()> {
        let Ok(mut killer_opt) = self.killer.lock() else {
            return Ok(());
        };
        let Some(killer) = killer_opt.as_mut() else {
            return Ok(());
        };

        killer.signal(signal)
    }

    /// Attempts to kill the child and abort I/O helper tasks.
    pub fn terminate(&self) {
        self.request_terminate();

        if let Ok(mut h) = self.reader_handle.lock()
            && let Some(handle) = h.take()
        {
            handle.abort();
        }
        if let Ok(mut handles) = self.reader_abort_handles.lock() {
            for handle in handles.drain(..) {
                handle.abort();
            }
        }
        if let Ok(mut h) = self.writer_handle.lock()
            && let Some(handle) = h.take()
        {
            handle.abort();
        }
        if let Ok(mut h) = self.wait_handle.lock()
            && let Some(handle) = h.take()
        {
            // Even a queued PTY waiter must run to reap the terminated child.
            drop(handle);
        }
    }
}

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        self.terminate();
    }
}

/// Adapts a closure into a `ChildTerminator` implementation.
struct ClosureTerminator {
    inner: Option<Box<dyn FnMut() + Send + Sync>>,
}

impl ChildTerminator for ClosureTerminator {
    fn signal(&mut self, signal: ProcessSignal) -> io::Result<()> {
        Err(unsupported_signal(signal))
    }

    fn kill(&mut self) -> io::Result<()> {
        if let Some(inner) = self.inner.as_mut() {
            (inner)();
        }
        Ok(())
    }
}

/// Combine split stdout/stderr receivers into a single broadcast receiver.
pub fn combine_output_receivers(
    mut stdout_rx: mpsc::Receiver<Vec<u8>>,
    mut stderr_rx: mpsc::Receiver<Vec<u8>>,
) -> broadcast::Receiver<Vec<u8>> {
    let (combined_tx, combined_rx) = broadcast::channel(256);
    tokio::spawn(async move {
        let mut stdout_open = true;
        let mut stderr_open = true;

        loop {
            tokio::select! {
                stdout = stdout_rx.recv(), if stdout_open => match stdout {
                    Some(chunk) => {
                        let _ = combined_tx.send(chunk);
                    }
                    None => {
                        stdout_open = false;
                    }
                },
                stderr = stderr_rx.recv(), if stderr_open => match stderr {
                    Some(chunk) => {
                        let _ = combined_tx.send(chunk);
                    }
                    None => {
                        stderr_open = false;
                    }
                },
                else => break,
            }
        }
    });
    combined_rx
}

/// Return value from PTY or pipe spawn helpers.
#[derive(Debug)]
pub struct SpawnedProcess {
    pub session: ProcessHandle,
    pub stdout_rx: mpsc::Receiver<Vec<u8>>,
    pub stderr_rx: mpsc::Receiver<Vec<u8>>,
    pub exit_rx: oneshot::Receiver<ProcessExit>,
}

/// Driver-backed process handles for non-standard spawn backends.
pub struct ProcessDriver {
    pub writer_tx: mpsc::Sender<Vec<u8>>,
    pub stdout_rx: broadcast::Receiver<Vec<u8>>,
    pub stderr_rx: Option<broadcast::Receiver<Vec<u8>>>,
    pub exit_rx: oneshot::Receiver<ProcessExit>,
    pub terminator: Option<Box<dyn FnMut() + Send + Sync>>,
    pub writer_handle: Option<JoinHandle<()>>,
    pub resizer: Option<ResizeFn>,
}

/// Build a `SpawnedProcess` from a driver that supplies stdin/output/exit channels.
pub fn spawn_from_driver(driver: ProcessDriver) -> SpawnedProcess {
    let ProcessDriver {
        writer_tx,
        stdout_rx: stdout_driver_rx,
        stderr_rx: mut stderr_driver_rx,
        exit_rx,
        terminator,
        writer_handle,
        resizer,
    } = driver;

    let (stdout_tx, stdout_rx) = mpsc::channel::<Vec<u8>>(256);
    let (stderr_tx, stderr_rx) = mpsc::channel::<Vec<u8>>(256);
    let (exit_seen_tx, exit_seen_rx) = watch::channel(false);
    let spawn_stream_reader =
        |mut output_rx: broadcast::Receiver<Vec<u8>>,
         output_tx: mpsc::Sender<Vec<u8>>,
         mut exit_seen_rx: watch::Receiver<bool>| {
            tokio::spawn(async move {
                loop {
                    let recv_result = if *exit_seen_rx.borrow() {
                        // Once exit has been observed, we no longer want a timer here. Some
                        // backends publish the exit code before their final stdout/stderr bytes
                        // have been forwarded through the broadcast channel, so a fixed grace
                        // period can still drop the tail of the stream under load.
                        //
                        // Instead, keep waiting until the driver closes the broadcast sender.
                        // That makes the shutdown contract explicit: the backend is responsible
                        // for dropping its sender when it has truly finished forwarding output.
                        output_rx.recv().await
                    } else {
                        tokio::select! {
                            _ = exit_seen_rx.changed() => {
                                continue;
                            }
                            result = output_rx.recv() => result,
                        }
                    };
                    match recv_result {
                        Ok(chunk) => {
                            if output_tx.send(chunk).await.is_err() {
                                break;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            })
        };
    let reader_handle = spawn_stream_reader(stdout_driver_rx, stdout_tx, exit_seen_rx.clone());
    let stderr_reader_handle = stderr_driver_rx
        .take()
        .map(|rx| spawn_stream_reader(rx, stderr_tx, exit_seen_rx));

    let writer_handle = writer_handle.unwrap_or_else(|| tokio::spawn(async {}));

    let (exit_tx, exit_rx_out) = oneshot::channel::<ProcessExit>();
    let exit_status = Arc::new(AtomicBool::new(false));
    let wait_exit_status = Arc::clone(&exit_status);
    let exit = Arc::new(StdMutex::new(None));
    let wait_exit = Arc::clone(&exit);
    let wait_handle = tokio::spawn(async move {
        let status = exit_rx.await.unwrap_or_else(|_| ProcessExit::exited(-1));
        wait_exit_status.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Ok(mut guard) = wait_exit.lock() {
            *guard = Some(status.clone());
        }
        let _ = exit_seen_tx.send(true);
        let _ = exit_tx.send(status);
    });

    let handle = ProcessHandle::new(
        writer_tx,
        Box::new(ClosureTerminator { inner: terminator }),
        reader_handle,
        stderr_reader_handle
            .map(|handle| handle.abort_handle())
            .into_iter()
            .collect(),
        writer_handle,
        wait_handle,
        exit_status,
        exit,
        /*pty_master*/ None,
        resizer,
    );

    SpawnedProcess {
        session: handle,
        stdout_rx,
        stderr_rx,
        exit_rx: exit_rx_out,
    }
}
