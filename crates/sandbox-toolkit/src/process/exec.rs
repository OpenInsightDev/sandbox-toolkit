//! exec starts a single executable with no shell in between: the program is one
//! executable token and its arguments are one argv entry each, so nothing is
//! re-parsed. [`CommandSpec`] is the shared core the shell format builds on.
//!
//! The response is the length-prefixed frame stream in [`super::frame`].

use std::{
    collections::HashMap,
    ffi::OsString,
    path::PathBuf,
    process::{ExitStatus, Stdio},
};

use thiserror::Error;
use tokio::{
    io::AsyncRead,
    process::{Child, ChildStderr, ChildStdout, Command},
    sync::mpsc,
};
use tokio_stream::{StreamExt, wrappers::ReceiverStream};
use tokio_util::io::ReaderStream;

use super::frame::Frame;
use super::model::{ExecCommon, ExecRequest, Status};
use crate::binary;
use crate::path::{self, PathError};
use crate::workspace::registry::Workspace;

/// Frames buffered before a slow reader back-pressures the process.
const FRAME_BUFFER: usize = 32;

const DEFAULT_SHELL: &str = "sh";

#[derive(Debug, Error)]
pub(crate) enum ExecError {
    #[error("command must not be empty")]
    EmptyCommand,
    #[error("invalid cwd `{cwd}`: {reason}")]
    InvalidCwd {
        cwd: String,
        /// For humans only.
        reason: String,
    },
    #[error("failed to run `{command}`: {source}")]
    Spawn {
        command: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to start a pty for `{command}`: {message}")]
    PtySpawn { command: String, message: String },
}

pub(crate) struct CommandSpec {
    pub(crate) program: String,
    pub(crate) args: Vec<String>,
    /// Always concrete: a request that omits `cwd` resolves to `.`, which inherits the
    /// server's directory, so spawning never has to special-case its absence.
    pub(crate) cwd: PathBuf,
    pub(crate) env: HashMap<String, OsString>,
}

/// The addressing mode's contribution: the workspace whose root confines `cwd`, if
/// any, and the variables every registered workspace exposes to the child.
#[derive(Debug, Default)]
pub(crate) struct WorkspaceContext {
    pub(crate) workspace: Option<Workspace>,
    pub(crate) environment: HashMap<String, OsString>,
}

pub(crate) async fn exec(
    request: ExecRequest,
    context: WorkspaceContext,
) -> Result<ReceiverStream<Frame>, ExecError> {
    let WorkspaceContext {
        workspace,
        environment,
    } = context;

    let spec = match request {
        ExecRequest::Exec {
            command,
            args,
            common: ExecCommon { cwd, env, .. },
        } => {
            if command.trim().is_empty() {
                return Err(ExecError::EmptyCommand);
            }

            CommandSpec::resolve(command, args, cwd, env, workspace.as_ref(), environment).await?
        }
        ExecRequest::Shell {
            script,
            shell,
            common: ExecCommon { cwd, env, .. },
        } => {
            let interpreter = shell.unwrap_or_else(|| DEFAULT_SHELL.to_owned());

            CommandSpec::resolve(
                interpreter,
                vec!["-c".to_owned(), script],
                cwd,
                env,
                workspace.as_ref(),
                environment,
            )
            .await?
        }
    };

    spec.spawn()
}

impl CommandSpec {
    pub(crate) async fn resolve(
        program: String,
        args: Vec<String>,
        cwd: Option<String>,
        env: HashMap<String, String>,
        workspace: Option<&Workspace>,
        environment: HashMap<String, OsString>,
    ) -> Result<Self, ExecError> {
        let cwd = match cwd.as_deref() {
            Some(cwd) => resolve_cwd(cwd, workspace).await?,
            None => workspace.map_or_else(|| PathBuf::from("."), |w| w.root().to_path_buf()),
        };

        // The injected workspace variables come first so a request variable of the
        // same name overrides them.
        let env = environment
            .into_iter()
            .chain(
                env.into_iter()
                    .map(|(name, value)| (name, OsString::from(value))),
            )
            .collect();

        Ok(Self {
            program,
            args,
            cwd,
            env,
        })
    }

    /// The child environment as strings, for the pty crate's string-only spawn
    /// API. The inherited environment is the base, so a pty session sees what the
    /// server does and then the request's overrides; `PATH` gets the materialized
    /// binaries directory first, exactly as the pipe backend sets it.
    pub(crate) fn environment(&self) -> HashMap<String, String> {
        let path = self
            .env
            .get("PATH")
            .cloned()
            .or_else(|| std::env::var_os("PATH"));

        let mut environment: HashMap<String, String> = std::env::vars_os()
            .map(|(name, value)| {
                (
                    name.to_string_lossy().into_owned(),
                    value.to_string_lossy().into_owned(),
                )
            })
            .collect();

        environment.extend(self.env.iter().map(|(name, value)| {
            (name.clone(), value.to_string_lossy().into_owned())
        }));
        environment.insert(
            "PATH".to_owned(),
            search_path(path).to_string_lossy().into_owned(),
        );

        environment
    }

    pub(crate) fn spawn(self) -> Result<ReceiverStream<Frame>, ExecError> {
        let path = self
            .env
            .get("PATH")
            .cloned()
            .or_else(|| std::env::var_os("PATH"));

        // `Command` is a `&mut self` builder, so `child` is the one binding that must
        // stay mutable; everything else is consumed by the chain.
        let mut child = Command::new(&self.program)
            .current_dir(&self.cwd)
            .args(&self.args)
            .envs(&self.env)
            .env("PATH", search_path(path))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Do not leave the process behind once the response stream is dropped.
            .kill_on_drop(true)
            .spawn()
            .map_err(|source| ExecError::Spawn {
                command: self.program,
                source,
            })?;

        let stdout = child.stdout.take().expect("stdout is piped");
        let stderr = child.stderr.take().expect("stderr is piped");
        let (sender, receiver) = mpsc::channel(FRAME_BUFFER);
        tokio::spawn(pump(child, stdout, stderr, sender));

        Ok(ReceiverStream::new(receiver))
    }
}

async fn resolve_cwd(cwd: &str, workspace: Option<&Workspace>) -> Result<PathBuf, ExecError> {
    let invalid = |reason: &str| ExecError::InvalidCwd {
        cwd: cwd.to_owned(),
        reason: reason.to_owned(),
    };

    // The same resolver as the filesystem API: relative and confined in workspace
    // mode, absolute in direct mode, with symlink escapes caught by canonicalizing
    // before the boundary check.
    let resolved = path::resolve(workspace, cwd)
        .await
        .map_err(|error| match error {
            PathError::InvalidPath { reason, .. } => invalid(&reason),
            PathError::NotFound(_) => invalid("cwd must be an existing directory"),
            PathError::Io(error) => invalid(&error.to_string()),
        })?;

    if !tokio::fs::metadata(&resolved)
        .await
        .is_ok_and(|meta| meta.is_dir())
    {
        return Err(invalid("cwd must be a directory"));
    }

    Ok(resolved)
}

/// The search path handed to the process: the materialized binaries directory first,
/// so bundled tools resolve by name, then whatever the caller or the server had.
fn search_path(inherited: Option<OsString>) -> OsString {
    // Startup fails without a cache directory, so the fallback only covers a
    // process that somehow spawns a child before materialization.
    let Ok(materialized) = binary::materialized_dir() else {
        return inherited.unwrap_or_default();
    };

    let Some(inherited) = inherited.filter(|path| !path.is_empty()) else {
        return materialized.into_os_string();
    };

    std::env::join_paths(std::iter::once(materialized).chain(std::env::split_paths(&inherited)))
        .unwrap_or(inherited)
}

async fn pump(
    mut child: Child,
    stdout: ChildStdout,
    stderr: ChildStderr,
    sender: mpsc::Sender<Frame>,
) {
    let out = tokio::spawn(forward(stdout, false, sender.clone()));
    let err = tokio::spawn(forward(stderr, true, sender.clone()));

    let status = tokio::select! {
        result = child.wait() => match result {
            Ok(status) => exit_status(status),
            Err(error) => Status::Failed { message: error.to_string() },
        },
        // The client is gone, so stop the process rather than let it run on; the
        // status frame is skipped because nobody can receive it.
        () = sender.closed() => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return;
        }
    };

    // A read error means the output is incomplete, which the status must report
    // rather than silently truncate; both pipes are drained first regardless.
    let failure = [out.await, err.await]
        .into_iter()
        .filter_map(|result| result.ok().and_then(Result::err))
        .next();

    let status = match failure {
        Some(message) => Status::Failed { message },
        None => status,
    };
    let _ = sender.send(Frame::Status(status)).await;
}

async fn forward<R>(reader: R, stderr: bool, sender: mpsc::Sender<Frame>) -> Result<(), String>
where
    R: AsyncRead + Unpin,
{
    let mut chunks = ReaderStream::new(reader);

    while let Some(chunk) = chunks.next().await {
        let payload = chunk.map_err(|error| error.to_string())?;
        let frame = if stderr {
            Frame::Stderr(payload)
        } else {
            Frame::Stdout(payload)
        };

        if sender.send(frame).await.is_err() {
            // The receiver side ends the process; nothing more can be sent.
            return Ok(());
        }
    }

    Ok(())
}

fn exit_status(status: ExitStatus) -> Status {
    match status.code() {
        Some(exit_code) => Status::Exited { exit_code },
        None => Status::Failed {
            message: "terminated by a signal".to_owned(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::model::WorkspaceProperties;
    use crate::workspace::registry::WorkspaceRegistry;
    use crate::workspace::registry::test_support::TempDir;

    fn request(command: &str, args: &[&str]) -> ExecRequest {
        exec_request(command, args, None, HashMap::new())
    }

    fn exec_request(
        command: &str,
        args: &[&str],
        cwd: Option<String>,
        env: HashMap<String, String>,
    ) -> ExecRequest {
        ExecRequest::Exec {
            command: command.to_owned(),
            args: args.iter().copied().map(str::to_owned).collect(),
            common: ExecCommon { cwd, env, wait: 0 },
        }
    }

    fn shell_request(script: &str) -> ExecRequest {
        shell_request_with(script, None, None)
    }

    fn shell_request_with(script: &str, shell: Option<String>, cwd: Option<String>) -> ExecRequest {
        ExecRequest::Shell {
            script: script.to_owned(),
            shell,
            common: ExecCommon {
                cwd,
                env: HashMap::new(),
                wait: 0,
            },
        }
    }

    async fn workspace(dir: &TempDir) -> Workspace {
        let registry = WorkspaceRegistry::default();
        registry
            .register("docs", &dir.root(), WorkspaceProperties::default())
            .await
            .unwrap();

        registry.workspace("docs").unwrap()
    }

    /// Addressing without any injected workspace variable.
    fn context(workspace: Option<Workspace>) -> WorkspaceContext {
        WorkspaceContext {
            workspace,
            environment: HashMap::new(),
        }
    }

    /// Run a request expected to spawn, collecting its whole response.
    async fn run(request: ExecRequest, workspace: Option<Workspace>) -> Vec<Frame> {
        run_with_env(request, workspace, HashMap::new()).await
    }

    async fn run_with_env(
        request: ExecRequest,
        workspace: Option<Workspace>,
        environment: HashMap<String, OsString>,
    ) -> Vec<Frame> {
        exec(
            request,
            WorkspaceContext {
                workspace,
                environment,
            },
        )
        .await
        .unwrap()
        .collect()
        .await
    }

    /// The concatenated bytes of one output channel.
    fn stream_bytes(frames: &[Frame], stderr: bool) -> Vec<u8> {
        frames
            .iter()
            .filter_map(|frame| match (frame, stderr) {
                (Frame::Stdout(bytes), false) | (Frame::Stderr(bytes), true) => {
                    Some(bytes.as_ref())
                }
                _ => None,
            })
            .flatten()
            .copied()
            .collect()
    }

    /// The terminal status, which must be the last frame.
    fn final_status(frames: &[Frame]) -> Status {
        match frames.last() {
            Some(Frame::Status(status)) => status.clone(),
            other => panic!("the stream did not end with a status: {other:?}"),
        }
    }

    #[tokio::test]
    async fn captures_stdout_and_reports_success() {
        let frames = run(request("printf", &["hello"]), None).await;

        assert_eq!(stream_bytes(&frames, false), b"hello");
        assert!(stream_bytes(&frames, true).is_empty());
        assert_eq!(final_status(&frames), Status::Exited { exit_code: 0 });
        assert_eq!(
            frames
                .iter()
                .filter(|frame| matches!(frame, Frame::Status(_)))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn separates_stderr_and_reports_a_nonzero_exit() {
        let frames = run(
            request("sh", &["-c", "printf out; printf err >&2; exit 3"]),
            None,
        )
        .await;

        assert_eq!(stream_bytes(&frames, false), b"out");
        assert_eq!(stream_bytes(&frames, true), b"err");
        assert_eq!(final_status(&frames), Status::Exited { exit_code: 3 });
    }

    #[tokio::test]
    async fn applies_a_direct_cwd_and_env() {
        let dir = TempDir::new();
        let request = exec_request(
            "sh",
            &["-c", "printf %s \"$GREETING\""],
            Some(dir.root()),
            HashMap::from([("GREETING".to_owned(), "hi".to_owned())]),
        );

        let frames = run(request, None).await;

        assert_eq!(stream_bytes(&frames, false), b"hi");
        assert_eq!(final_status(&frames), Status::Exited { exit_code: 0 });
    }

    #[tokio::test]
    async fn injects_the_workspace_variable_and_confines_a_relative_cwd() {
        let dir = TempDir::new();
        let workspace = workspace(&dir).await;
        let request = exec_request(
            "sh",
            &["-c", "printf %s \"$WORKSPACE_DOCS\""],
            Some(".".to_owned()),
            HashMap::new(),
        );

        let environment = HashMap::from([workspace.env()]);
        let frames = run_with_env(request, Some(workspace.clone()), environment).await;

        assert_eq!(
            String::from_utf8(stream_bytes(&frames, false)).unwrap(),
            workspace.root().display().to_string()
        );
        assert_eq!(final_status(&frames), Status::Exited { exit_code: 0 });
    }

    #[tokio::test]
    async fn a_request_variable_overrides_the_workspace_one() {
        let dir = TempDir::new();
        let request = exec_request(
            "sh",
            &["-c", "printf %s \"$WORKSPACE_DOCS\""],
            None,
            HashMap::from([("WORKSPACE_DOCS".to_owned(), "override".to_owned())]),
        );

        let workspace = workspace(&dir).await;
        let environment = HashMap::from([workspace.env()]);
        let frames = run_with_env(request, Some(workspace), environment).await;

        assert_eq!(stream_bytes(&frames, false), b"override");
    }

    #[tokio::test]
    async fn rejects_a_cwd_that_escapes_the_workspace() {
        let dir = TempDir::new();
        let request = exec_request("true", &[], Some("../outside".to_owned()), HashMap::new());

        assert!(matches!(
            exec(request, context(Some(workspace(&dir).await))).await,
            Err(ExecError::InvalidCwd { .. })
        ));
    }

    #[tokio::test]
    async fn rejects_a_relative_cwd_in_direct_mode() {
        let request = exec_request("true", &[], Some("relative".to_owned()), HashMap::new());

        assert!(matches!(
            exec(request, context(None)).await,
            Err(ExecError::InvalidCwd { .. })
        ));
    }

    #[tokio::test]
    async fn rejects_an_empty_command() {
        assert!(matches!(
            exec(request("  ", &[]), context(None)).await,
            Err(ExecError::EmptyCommand)
        ));
    }

    #[tokio::test]
    async fn reports_a_missing_program_as_a_spawn_error() {
        assert!(matches!(
            exec(request("definitely-not-a-real-command", &[]), context(None)).await,
            Err(ExecError::Spawn { .. })
        ));
    }

    #[tokio::test]
    async fn shell_runs_a_script_through_the_default_interpreter() {
        let frames = run(shell_request("printf out; printf err >&2; exit 4"), None).await;

        assert_eq!(stream_bytes(&frames, false), b"out");
        assert_eq!(stream_bytes(&frames, true), b"err");
        assert_eq!(final_status(&frames), Status::Exited { exit_code: 4 });
    }

    #[tokio::test]
    async fn reports_an_unknown_shell_as_a_spawn_error() {
        let request =
            shell_request_with("true", Some("definitely-not-a-real-shell".to_owned()), None);

        assert!(matches!(
            exec(request, context(None)).await,
            Err(ExecError::Spawn { .. })
        ));
    }

    #[tokio::test]
    async fn shell_shares_the_workspace_boundary() {
        let dir = TempDir::new();
        let request = shell_request_with("true", None, Some("../outside".to_owned()));

        assert!(matches!(
            exec(request, context(Some(workspace(&dir).await))).await,
            Err(ExecError::InvalidCwd { .. })
        ));
    }
}
