use std::collections::HashMap;
use std::io::ErrorKind;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicBool;

use anyhow::Result;
use portable_pty::native_pty_system;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::process::ChildTerminator;
use crate::process::ProcessExit;
use crate::process::ProcessHandle;
use crate::process::ProcessSignal;
use crate::process::SpawnedProcess;
use crate::process::TerminalSize;

struct PtyChildTerminator {
    killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
    // portable-pty establishes the spawned PTY child as a new session leader, so
    // PID == PGID and the pipe backend's process-group kill semantics apply.
    process_group_id: Option<u32>,
}

impl ChildTerminator for PtyChildTerminator {
    fn signal(&mut self, signal: ProcessSignal) -> std::io::Result<()> {
        match signal {
            ProcessSignal::Interrupt => {
                if let Some(process_group_id) = self.process_group_id {
                    return crate::process_group::interrupt_process_group(process_group_id);
                }

                Err(crate::process::unsupported_signal(signal))
            }
        }
    }

    fn kill(&mut self) -> std::io::Result<()> {
        if let Some(process_group_id) = self.process_group_id {
            // Match the pipe backend's hard-kill behavior so descendant
            // processes from interactive shells/REPLs do not survive shutdown.
            // Also try the direct child killer in case the cached PGID is stale.
            let process_group_kill_result =
                crate::process_group::kill_process_group(process_group_id);
            let child_kill_result = self.killer.kill();
            return match child_kill_result {
                Ok(()) => Ok(()),
                Err(err) if err.kind() == ErrorKind::NotFound => process_group_kill_result,
                Err(err) => process_group_kill_result.or(Err(err)),
            };
        }

        self.killer.kill()
    }
}

/// Spawn a process attached to a PTY.
pub async fn spawn_process(
    program: &str,
    args: &[String],
    cwd: &Path,
    env: &HashMap<String, String>,
    arg0: &Option<String>,
    size: TerminalSize,
) -> Result<SpawnedProcess> {
    if program.is_empty() {
        anyhow::bail!("missing program for PTY spawn");
    }

    let pty_system = native_pty_system();
    let pair = pty_system.openpty(size.into())?;
    let io = crate::unix_io::PtyIo::new(
        pair.master
            .as_raw_fd()
            .ok_or_else(|| anyhow::anyhow!("PTY master has no file descriptor"))?,
    )?;

    let slave_path = pair
        .master
        .tty_name()
        .ok_or_else(|| anyhow::anyhow!("PTY master has no slave device path"))?;
    // The parent must not acquire the slave as its own controlling terminal;
    // the child claims it with `TIOCSCTTY` after `setsid`.
    let slave = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOCTTY)
        .open(&slave_path)?;

    let program_arg = arg0.clone().unwrap_or_else(|| program.to_string());
    let mut command = Command::new(&program_arg);
    command.current_dir(cwd);
    command.env_clear();
    for arg in args {
        command.arg(arg);
    }
    for (key, value) in env {
        command.env(key, value);
    }

    #[cfg(target_os = "linux")]
    let parent_pid = unsafe { libc::getpid() };
    unsafe {
        command
            .stdin(Stdio::from(slave.try_clone()?))
            .stdout(Stdio::from(slave.try_clone()?))
            .stderr(Stdio::from(slave.try_clone()?))
            .pre_exec(move || {
                // portable-pty clears inherited dispositions and starts a new
                // session so the slave becomes the child's controlling terminal.
                for signo in &[
                    libc::SIGCHLD,
                    libc::SIGHUP,
                    libc::SIGINT,
                    libc::SIGQUIT,
                    libc::SIGTERM,
                    libc::SIGALRM,
                ] {
                    libc::signal(*signo, libc::SIG_DFL);
                }

                let empty_set: libc::sigset_t = std::mem::zeroed();
                libc::sigprocmask(libc::SIG_SETMASK, &empty_set, std::ptr::null_mut());

                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }

                #[allow(clippy::cast_lossless)]
                if libc::ioctl(0, libc::TIOCSCTTY as _, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }

                #[cfg(target_os = "linux")]
                crate::process_group::set_parent_death_signal(parent_pid)?;

                Ok(())
            });
    }

    let mut child = command.spawn()?;
    drop(slave);
    let process_group_id = portable_pty::Child::process_id(&child);
    let killer = portable_pty::ChildKiller::clone_killer(&child);

    let (writer_tx, writer_rx) = mpsc::channel::<Vec<u8>>(128);
    let (stdout_tx, stdout_rx) = mpsc::channel::<Vec<u8>>(128);
    let (_stderr_tx, stderr_rx) = mpsc::channel::<Vec<u8>>(1);
    let (reader_handle, writer_handle) = io.spawn(stdout_tx, writer_rx);

    let (exit_tx, exit_rx) = oneshot::channel::<ProcessExit>();
    let exit_status = Arc::new(AtomicBool::new(false));
    let wait_exit_status = Arc::clone(&exit_status);
    let exit = Arc::new(StdMutex::new(None));
    let wait_exit = Arc::clone(&exit);
    let wait_handle: JoinHandle<()> = tokio::task::spawn_blocking(move || {
        let status = match portable_pty::Child::wait(&mut child) {
            Ok(status) => match status.signal() {
                Some(signal) => ProcessExit::signaled(status.exit_code() as i32, signal),
                None => ProcessExit::exited(status.exit_code() as i32),
            },
            Err(_) => ProcessExit::exited(-1),
        };
        wait_exit_status.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Ok(mut guard) = wait_exit.lock() {
            *guard = Some(status.clone());
        }
        let _ = exit_tx.send(status);
    });

    let handle = ProcessHandle::new(
        writer_tx,
        Box::new(PtyChildTerminator {
            killer,
            process_group_id,
        }),
        reader_handle,
        Vec::new(),
        writer_handle,
        wait_handle,
        exit_status,
        exit,
        Some(pair.master),
        /*resizer*/ None,
    );

    Ok(SpawnedProcess {
        session: handle,
        stdout_rx,
        stderr_rx,
        exit_rx,
    })
}
