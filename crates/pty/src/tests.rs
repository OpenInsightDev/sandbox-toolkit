use std::collections::HashMap;
use std::path::Path;

use crate::SpawnedProcess;
use crate::TerminalSize;
use crate::combine_output_receivers;
use crate::spawn_pipe_process;
use crate::spawn_pipe_process_no_stdin;
use crate::spawn_pty_process;

fn env() -> HashMap<String, String> {
    std::env::vars().collect()
}

fn sh(script: &str) -> (String, Vec<String>) {
    (
        "/bin/sh".to_string(),
        vec!["-c".to_string(), script.to_string()],
    )
}

/// Collect the combined stdout/stderr stream until the process exits, or until
/// the timeout elapses.
async fn collect_until_exit(
    mut output_rx: tokio::sync::broadcast::Receiver<Vec<u8>>,
    exit_rx: tokio::sync::oneshot::Receiver<i32>,
    timeout: std::time::Duration,
) -> (Vec<u8>, Option<i32>) {
    let mut collected = Vec::new();
    let deadline = tokio::time::Instant::now() + timeout;
    tokio::pin!(exit_rx);

    loop {
        tokio::select! {
            res = output_rx.recv() => if let Ok(chunk) = res {
                collected.extend_from_slice(&chunk);
            },
            res = &mut exit_rx => return (collected, res.ok()),
            () = tokio::time::sleep_until(deadline) => return (collected, None),
        }
    }
}

async fn collect_split(mut rx: tokio::sync::mpsc::Receiver<Vec<u8>>) -> Vec<u8> {
    let mut collected = Vec::new();
    while let Some(chunk) = rx.recv().await {
        collected.extend_from_slice(&chunk);
    }
    collected
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pty_emits_output_and_exits() -> anyhow::Result<()> {
    let (program, args) = sh("printf pty_ok");
    let spawned = spawn_pty_process(
        &program,
        &args,
        Path::new("."),
        &env(),
        &None,
        TerminalSize::default(),
    )
    .await?;
    let SpawnedProcess {
        session: _session,
        stdout_rx,
        stderr_rx,
        exit_rx,
    } = spawned;

    let (output, code) = collect_until_exit(
        combine_output_receivers(stdout_rx, stderr_rx),
        exit_rx,
        secs(5),
    )
    .await;

    assert_eq!(code, Some(0));
    assert!(String::from_utf8_lossy(&output).contains("pty_ok"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pty_round_trips_stdin_and_eof() -> anyhow::Result<()> {
    // `cat` echoes stdin back and only exits once it sees EOF, which closing the
    // stdin channel must deliver.
    let (program, args) = sh("cat");
    let spawned = spawn_pty_process(
        &program,
        &args,
        Path::new("."),
        &env(),
        &None,
        TerminalSize::default(),
    )
    .await?;
    let SpawnedProcess {
        session,
        stdout_rx,
        stderr_rx,
        exit_rx,
    } = spawned;

    let writer = session.writer_sender();
    writer.send(b"round-trip\n".to_vec()).await?;
    drop(writer);
    session.close_stdin();

    let (output, code) = collect_until_exit(
        combine_output_receivers(stdout_rx, stderr_rx),
        exit_rx,
        secs(5),
    )
    .await;

    assert_eq!(code, Some(0));
    assert!(String::from_utf8_lossy(&output).contains("round-trip"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pty_resize_has_no_backend_error() -> anyhow::Result<()> {
    let (program, args) = sh("sleep 60");
    let spawned = spawn_pty_process(
        &program,
        &args,
        Path::new("."),
        &env(),
        &None,
        TerminalSize::default(),
    )
    .await?;

    spawned.session.resize(TerminalSize {
        rows: 40,
        cols: 120,
    })?;
    spawned.session.terminate();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pipe_round_trips_stdin() -> anyhow::Result<()> {
    let (program, args) = sh("cat");
    let spawned = spawn_pipe_process(&program, &args, Path::new("."), &env(), &None).await?;
    let SpawnedProcess {
        session,
        stdout_rx,
        stderr_rx,
        exit_rx,
    } = spawned;

    let writer = session.writer_sender();
    writer.send(b"roundtrip\n".to_vec()).await?;
    drop(writer);
    session.close_stdin();

    let (output, code) = collect_until_exit(
        combine_output_receivers(stdout_rx, stderr_rx),
        exit_rx,
        secs(5),
    )
    .await;

    assert_eq!(code, Some(0));
    assert!(String::from_utf8_lossy(&output).contains("roundtrip"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pipe_exposes_split_stdout_and_stderr() -> anyhow::Result<()> {
    let (program, args) = sh("printf out; printf err >&2");
    let spawned =
        spawn_pipe_process_no_stdin(&program, &args, Path::new("."), &env(), &None).await?;
    let SpawnedProcess {
        session: _session,
        stdout_rx,
        stderr_rx,
        exit_rx,
    } = spawned;

    let stdout_task = tokio::spawn(collect_split(stdout_rx));
    let stderr_task = tokio::spawn(collect_split(stderr_rx));
    let code = tokio::time::timeout(secs(5), exit_rx).await??;
    let stdout = stdout_task.await?;
    let stderr = stderr_task.await?;

    assert_eq!(stdout, b"out");
    assert_eq!(stderr, b"err");
    assert_eq!(code, 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminate_reaps_child() -> anyhow::Result<()> {
    let (program, args) = sh("sleep 60");
    let spawned = spawn_pipe_process(&program, &args, Path::new("."), &env(), &None).await?;
    let SpawnedProcess {
        session,
        stdout_rx: _stdout_rx,
        stderr_rx: _stderr_rx,
        exit_rx,
    } = spawned;

    session.terminate();

    let code = tokio::time::timeout(secs(5), exit_rx).await??;
    assert_eq!(session.exit_code(), Some(code));
    assert!(session.has_exited());
    Ok(())
}

fn secs(seconds: u64) -> std::time::Duration {
    std::time::Duration::from_secs(seconds)
}
