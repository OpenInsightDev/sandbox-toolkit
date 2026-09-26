use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicBool;

use anyhow::Result;
use portable_pty::CommandBuilder;
use portable_pty::native_pty_system;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::process::ChildTerminator;
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

    let mut command_builder = CommandBuilder::new(arg0.as_ref().unwrap_or(&program.to_string()));
    command_builder.cwd(cwd);
    command_builder.env_clear();
    for arg in args {
        command_builder.arg(arg);
    }
    for (key, value) in env {
        command_builder.env(key, value);
    }

    let mut child = pair.slave.spawn_command(command_builder)?;
    let process_group_id = child.process_id();
    let killer = child.clone_killer();

    let (writer_tx, writer_rx) = mpsc::channel::<Vec<u8>>(128);
    let (stdout_tx, stdout_rx) = mpsc::channel::<Vec<u8>>(128);
    let (_stderr_tx, stderr_rx) = mpsc::channel::<Vec<u8>>(1);
    let (reader_handle, writer_handle) = io.spawn(stdout_tx, writer_rx);

    let (exit_tx, exit_rx) = oneshot::channel::<i32>();
    let exit_status = Arc::new(AtomicBool::new(false));
    let wait_exit_status = Arc::clone(&exit_status);
    let exit_code = Arc::new(StdMutex::new(None));
    let wait_exit_code = Arc::clone(&exit_code);
    let wait_handle: JoinHandle<()> = tokio::task::spawn_blocking(move || {
        let code = match child.wait() {
            Ok(status) => status.exit_code() as i32,
            Err(_) => -1,
        };
        wait_exit_status.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Ok(mut guard) = wait_exit_code.lock() {
            *guard = Some(code);
        }
        let _ = exit_tx.send(code);
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
        exit_code,
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
