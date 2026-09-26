//! exec starts a single executable with no shell in between: the program is one
//! executable token and its arguments are one argv entry each, so nothing is
//! re-parsed. [`CommandSpec`] is the shared core that shell builds on.
//!
//! Frames are length-prefixed rather than separated, because command output may
//! contain any byte a separator would use. Every stream ends with exactly one
//! terminal [`Frame::Status`]; without it the stream was truncated, not successful,
//! which [`FrameStream`] enforces.

#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the frame decoders serve clients and tests, never the server"
    )
)]

use std::{
    collections::HashMap,
    ffi::OsString,
    path::{Component, Path, PathBuf},
    process::{ExitStatus, Stdio},
};

use bytes::{Buf, BufMut, Bytes, BytesMut};
use thiserror::Error;
use tokio::{
    io::AsyncRead,
    process::{Child, ChildStderr, ChildStdout, Command},
    sync::mpsc,
};
use tokio_stream::{StreamExt, wrappers::ReceiverStream};
use tokio_util::io::ReaderStream;

use super::model::{ExecRequest, ShellRequest, Status};
use crate::binary;
use crate::workspace::registry::WorkspaceEnvironment;

/// A frame header: one channel byte and a four-byte big-endian length.
pub(crate) const HEADER_LEN: usize = 5;

/// The largest payload a frame may carry.
///
/// The peer controls the advertised length, so [`Frame::decode`] rejects anything
/// larger before allocating.
pub(crate) const MAX_PAYLOAD_LEN: usize = 4 * 1024 * 1024;

/// The exec stream is server to client only: an exec request carries no input and
/// no terminal, so the only channels are the two output streams and the terminal
/// status.
///
/// The discriminants are wire identifiers; reordering them changes the format.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Channel {
    Stdout = 0,
    Stderr = 1,
    /// The terminal status, carrying success as well as failure.
    Error = 2,
}

impl Channel {
    pub(crate) const fn as_u8(self) -> u8 {
        self as u8
    }

    pub(crate) fn from_u8(id: u8) -> Result<Self, FrameError> {
        match id {
            0 => Ok(Self::Stdout),
            1 => Ok(Self::Stderr),
            2 => Ok(Self::Error),
            other => Err(FrameError::UnknownChannel(other)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Frame {
    Stdout(Bytes),
    Stderr(Bytes),
    /// On the error channel. Exactly one ends every stream, whether the command
    /// succeeded or failed.
    Status(Status),
}

impl Frame {
    pub(crate) const fn channel(&self) -> Channel {
        match self {
            Self::Stdout(_) => Channel::Stdout,
            Self::Stderr(_) => Channel::Stderr,
            Self::Status(_) => Channel::Error,
        }
    }

    pub(crate) fn payload_len(&self) -> usize {
        match self {
            Self::Stdout(bytes) | Self::Stderr(bytes) => bytes.len(),
            Self::Status(status) => serde_json::to_vec(status)
                .expect("a status frame always serializes to JSON")
                .len(),
        }
    }

    /// Encodes the frame. Splitting payloads above [`MAX_PAYLOAD_LEN`] is the
    /// producer's responsibility, since only the decoder enforces that bound.
    pub(crate) fn encode(&self) -> Bytes {
        let payload = self.payload();
        let mut buffer = BytesMut::with_capacity(HEADER_LEN + payload.len());
        buffer.put_u8(self.channel().as_u8());
        buffer.put_u32(payload.len() as u32);
        buffer.put_slice(&payload);
        buffer.freeze()
    }

    /// Removes the first frame from `buffer`, or returns `None` while `buffer`
    /// holds less than a complete frame, leaving it untouched for a later retry.
    ///
    /// This is the raw reader; it does not track the terminal [`Frame::Status`].
    pub(crate) fn decode(buffer: &mut BytesMut) -> Result<Option<Self>, FrameError> {
        let Some(header) = buffer.get(..HEADER_LEN) else {
            return Ok(None);
        };

        let channel = Channel::from_u8(header[0])?;
        let length = u32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize;
        if length > MAX_PAYLOAD_LEN {
            return Err(FrameError::PayloadTooLarge(length));
        }
        if buffer.len() < HEADER_LEN + length {
            return Ok(None);
        }

        buffer.advance(HEADER_LEN);
        let payload = buffer.split_to(length).freeze();
        Self::from_payload(channel, payload).map(Some)
    }

    fn payload(&self) -> Bytes {
        match self {
            Self::Stdout(bytes) | Self::Stderr(bytes) => bytes.clone(),
            Self::Status(status) => serde_json::to_vec(status)
                .expect("a status frame always serializes to JSON")
                .into(),
        }
    }

    fn from_payload(channel: Channel, payload: Bytes) -> Result<Self, FrameError> {
        let frame = match channel {
            Channel::Stdout => Self::Stdout(payload),
            Channel::Stderr => Self::Stderr(payload),
            Channel::Error => Self::Status(
                serde_json::from_slice(payload.as_ref()).map_err(FrameError::InvalidStatus)?,
            ),
        };

        Ok(frame)
    }
}

/// Enforces the terminal [`Frame::Status`] rule that [`Frame::decode`] cannot: the
/// status frame is last, and its absence means the stream was truncated.
#[derive(Debug, Default)]
pub(crate) struct FrameStream {
    buffer: BytesMut,
    finished: bool,
}

impl FrameStream {
    pub(crate) fn feed(&mut self, chunk: &[u8]) {
        self.buffer.extend_from_slice(chunk);
    }

    /// Returns the next frame, or `None` when more bytes are needed. A call after
    /// the status frame is an error, so readers stop at [`Frame::Status`].
    pub(crate) fn decode(&mut self) -> Result<Option<Frame>, FrameError> {
        if self.finished {
            return Err(FrameError::FrameAfterStatus);
        }

        let frame = Frame::decode(&mut self.buffer)?;
        if matches!(frame, Some(Frame::Status(_))) {
            self.finished = true;
        }

        Ok(frame)
    }

    pub(crate) const fn is_finished(&self) -> bool {
        self.finished
    }

    /// Errors if the stream ended before its status frame arrived.
    pub(crate) fn finish(self) -> Result<(), FrameError> {
        if self.finished {
            Ok(())
        } else {
            Err(FrameError::MissingStatus)
        }
    }
}

#[derive(Debug, Error)]
pub(crate) enum FrameError {
    #[error("unknown channel id: {0}")]
    UnknownChannel(u8),
    #[error("status frame payload is not valid JSON")]
    InvalidStatus(#[source] serde_json::Error),
    #[error("frame payload of {0} bytes exceeds the per-frame limit")]
    PayloadTooLarge(usize),
    #[error("frame received after the terminal status frame")]
    FrameAfterStatus,
    #[error("stream ended without a terminal status frame")]
    MissingStatus,
}

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
}

pub(crate) struct CommandSpec {
    pub(crate) program: String,
    pub(crate) args: Vec<String>,
    /// Always concrete: a request that omits `cwd` resolves to `.`, which inherits the
    /// server's directory, so spawning never has to special-case its absence.
    pub(crate) cwd: PathBuf,
    pub(crate) env: HashMap<String, OsString>,
}

pub(crate) async fn exec(
    request: ExecRequest,
    workspace: Option<WorkspaceEnvironment>,
) -> Result<ReceiverStream<Frame>, ExecError> {
    CommandSpec::from_exec(request, workspace.as_ref())
        .await?
        .spawn()
}

pub(crate) async fn shell(
    request: ShellRequest,
    workspace: Option<WorkspaceEnvironment>,
) -> Result<ReceiverStream<Frame>, ExecError> {
    CommandSpec::from_shell(request, workspace.as_ref())
        .await?
        .spawn()
}

impl CommandSpec {
    async fn from_exec(
        request: ExecRequest,
        workspace: Option<&WorkspaceEnvironment>,
    ) -> Result<Self, ExecError> {
        if request.command.trim().is_empty() {
            return Err(ExecError::EmptyCommand);
        }

        Self::resolve(
            request.command,
            request.args,
            request.cwd,
            request.env,
            workspace,
        )
        .await
    }

    async fn from_shell(
        request: ShellRequest,
        workspace: Option<&WorkspaceEnvironment>,
    ) -> Result<Self, ExecError> {
        let interpreter = request.shell.unwrap_or_else(|| DEFAULT_SHELL.to_owned());

        Self::resolve(
            interpreter,
            vec!["-c".to_owned(), request.script],
            request.cwd,
            request.env,
            workspace,
        )
        .await
    }

    async fn resolve(
        program: String,
        args: Vec<String>,
        cwd: Option<String>,
        env: HashMap<String, String>,
        workspace: Option<&WorkspaceEnvironment>,
    ) -> Result<Self, ExecError> {
        let cwd = match cwd.as_deref() {
            Some(cwd) => resolve_cwd(cwd, workspace).await?,
            None => workspace.map_or_else(|| PathBuf::from("."), |w| w.value.clone()),
        };

        // The workspace variable comes first so a request variable of the same name
        // overrides it.
        let env = workspace
            .iter()
            .map(|workspace| {
                (
                    workspace.name.clone(),
                    workspace.value.clone().into_os_string(),
                )
            })
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

    /// Start the process with piped output and return the frames of its response.
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

async fn resolve_cwd(
    cwd: &str,
    workspace: Option<&WorkspaceEnvironment>,
) -> Result<PathBuf, ExecError> {
    let invalid = |reason: &str| ExecError::InvalidCwd {
        cwd: cwd.to_owned(),
        reason: reason.to_owned(),
    };

    let Some(workspace) = workspace else {
        let path = PathBuf::from(cwd);
        if !path.is_absolute() {
            return Err(invalid("cwd must be absolute in direct mode"));
        }
        if !tokio::fs::metadata(&path)
            .await
            .is_ok_and(|meta| meta.is_dir())
        {
            return Err(invalid("cwd must be an existing directory"));
        }
        return Ok(path);
    };

    // Workspace mode: `cwd` is relative and confined to the root, so it is joined and
    // then canonicalized the same way file paths are, which catches symlink escapes.
    let relative = Path::new(cwd);
    if relative.is_absolute() {
        return Err(invalid("cwd must be relative in workspace mode"));
    }
    if relative
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(invalid("cwd must not contain `..`"));
    }

    let root = tokio::fs::canonicalize(&workspace.value)
        .await
        .map_err(|_| invalid("workspace root is not readable"))?;
    let target = tokio::fs::canonicalize(root.join(relative))
        .await
        .map_err(|_| invalid("cwd must be an existing directory"))?;

    if !target.starts_with(&root) {
        return Err(invalid("cwd escapes the workspace"));
    }
    if !target.is_dir() {
        return Err(invalid("cwd must be a directory"));
    }

    Ok(target)
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
        Some(0) => Status::Success,
        Some(code) => Status::Exited { code },
        None => Status::Failed {
            message: "terminated by a signal".to_owned(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::registry::test_support::{TempDir, canonical};

    fn samples() -> Vec<Frame> {
        vec![
            Frame::Stdout(Bytes::from_static(b"out")),
            Frame::Stderr(Bytes::from_static(b"err")),
            Frame::Status(Status::Exited { code: 3 }),
        ]
    }

    #[test]
    fn channel_ids_are_wire_stable() {
        assert_eq!(Channel::Stdout.as_u8(), 0);
        assert_eq!(Channel::Stderr.as_u8(), 1);
        assert_eq!(Channel::Error.as_u8(), 2);
    }

    #[test]
    fn round_trips_every_frame() {
        for frame in samples() {
            let mut buffer = BytesMut::from(frame.encode().as_ref());
            assert_eq!(Frame::decode(&mut buffer).unwrap(), Some(frame));
            assert!(buffer.is_empty());
        }
    }

    #[test]
    fn round_trips_every_status() {
        let statuses = [
            Status::Success,
            Status::Exited { code: 1 },
            Status::Failed {
                message: "spawn failed".to_owned(),
            },
        ];

        for status in statuses {
            let frame = Frame::Status(status);
            let mut buffer = BytesMut::from(frame.encode().as_ref());
            assert_eq!(Frame::decode(&mut buffer).unwrap(), Some(frame));
            assert!(buffer.is_empty());
        }
    }

    #[test]
    fn decodes_concatenated_frames() {
        let frames = samples();
        let mut buffer = BytesMut::new();
        for frame in &frames {
            buffer.extend_from_slice(&frame.encode());
        }

        for frame in frames {
            assert_eq!(Frame::decode(&mut buffer).unwrap(), Some(frame));
        }
        assert!(buffer.is_empty());
    }

    #[test]
    fn waits_until_a_frame_is_complete() {
        let encoded = Frame::Stdout(Bytes::from_static(b"hello")).encode();

        for prefix in 0..encoded.len() {
            let mut buffer = BytesMut::from(&encoded[..prefix]);
            assert_eq!(Frame::decode(&mut buffer).unwrap(), None);
            assert_eq!(buffer.len(), prefix, "an incomplete frame is left in place");
        }
    }

    #[test]
    fn rejects_an_unknown_channel() {
        let mut buffer = BytesMut::new();
        buffer.put_u8(3);
        buffer.put_u32(0);

        assert!(matches!(
            Frame::decode(&mut buffer),
            Err(FrameError::UnknownChannel(3))
        ));
    }

    #[test]
    fn rejects_an_oversized_payload() {
        let mut buffer = BytesMut::new();
        buffer.put_u8(Channel::Stdout.as_u8());
        buffer.put_u32(MAX_PAYLOAD_LEN as u32 + 1);

        assert!(matches!(
            Frame::decode(&mut buffer),
            Err(FrameError::PayloadTooLarge(_))
        ));
    }

    #[test]
    fn rejects_a_malformed_status_frame() {
        let mut buffer = BytesMut::new();
        buffer.put_u8(Channel::Error.as_u8());
        buffer.put_u32(3);
        buffer.put_slice(b"n/a");

        assert!(matches!(
            Frame::decode(&mut buffer),
            Err(FrameError::InvalidStatus(_))
        ));
    }

    #[test]
    fn accepts_a_stream_that_ends_with_a_status_frame() {
        let mut stream = FrameStream::default();
        stream.feed(&Frame::Stdout(Bytes::from_static(b"out")).encode());
        stream.feed(&Frame::Status(Status::Success).encode());

        assert_eq!(
            stream.decode().unwrap(),
            Some(Frame::Stdout(Bytes::from_static(b"out")))
        );
        assert!(!stream.is_finished());

        assert_eq!(
            stream.decode().unwrap(),
            Some(Frame::Status(Status::Success))
        );
        assert!(stream.is_finished());

        stream.finish().unwrap();
    }

    #[test]
    fn rejects_a_frame_after_the_status_frame() {
        let mut stream = FrameStream::default();
        stream.feed(&Frame::Status(Status::Success).encode());
        stream.feed(&Frame::Stdout(Bytes::from_static(b"late")).encode());

        assert!(matches!(stream.decode().unwrap(), Some(Frame::Status(_))));
        assert!(matches!(stream.decode(), Err(FrameError::FrameAfterStatus)));
    }

    #[test]
    fn rejects_a_stream_without_a_status_frame() {
        let mut stream = FrameStream::default();
        stream.feed(&Frame::Stderr(Bytes::from_static(b"truncated")).encode());

        assert!(matches!(stream.decode().unwrap(), Some(Frame::Stderr(_))));
        assert!(matches!(stream.finish(), Err(FrameError::MissingStatus)));
    }

    fn request(command: &str, args: &[&str]) -> ExecRequest {
        ExecRequest {
            command: command.to_owned(),
            args: args.iter().copied().map(str::to_owned).collect(),
            cwd: None,
            env: HashMap::new(),
            timeout: None,
        }
    }

    fn workspace(dir: &TempDir) -> WorkspaceEnvironment {
        WorkspaceEnvironment {
            name: "WORKSPACE_DOCS".to_owned(),
            value: canonical(dir.path()),
        }
    }

    /// Run a request expected to spawn, collecting its whole response.
    async fn run(request: ExecRequest, workspace: Option<WorkspaceEnvironment>) -> Vec<Frame> {
        exec(request, workspace).await.unwrap().collect().await
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
        assert_eq!(final_status(&frames), Status::Success);
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
        assert_eq!(final_status(&frames), Status::Exited { code: 3 });
    }

    #[tokio::test]
    async fn applies_a_direct_cwd_and_env() {
        let dir = TempDir::new();
        let request = ExecRequest {
            cwd: Some(dir.root()),
            env: HashMap::from([("GREETING".to_owned(), "hi".to_owned())]),
            ..request("sh", &["-c", "printf %s \"$GREETING\""])
        };

        let frames = run(request, None).await;

        assert_eq!(stream_bytes(&frames, false), b"hi");
        assert_eq!(final_status(&frames), Status::Success);
    }

    #[tokio::test]
    async fn injects_the_workspace_variable_and_confines_a_relative_cwd() {
        let dir = TempDir::new();
        let workspace = workspace(&dir);
        let request = ExecRequest {
            cwd: Some(".".to_owned()),
            ..request("sh", &["-c", "printf %s \"$WORKSPACE_DOCS\""])
        };

        let frames = run(request, Some(workspace.clone())).await;

        assert_eq!(
            String::from_utf8(stream_bytes(&frames, false)).unwrap(),
            workspace.value.display().to_string()
        );
        assert_eq!(final_status(&frames), Status::Success);
    }

    #[tokio::test]
    async fn a_request_variable_overrides_the_workspace_one() {
        let dir = TempDir::new();
        let request = ExecRequest {
            env: HashMap::from([("WORKSPACE_DOCS".to_owned(), "override".to_owned())]),
            ..request("sh", &["-c", "printf %s \"$WORKSPACE_DOCS\""])
        };

        let frames = run(request, Some(workspace(&dir))).await;

        assert_eq!(stream_bytes(&frames, false), b"override");
    }

    #[tokio::test]
    async fn rejects_a_cwd_that_escapes_the_workspace() {
        let dir = TempDir::new();
        let request = ExecRequest {
            cwd: Some("../outside".to_owned()),
            ..request("true", &[])
        };

        assert!(matches!(
            exec(request, Some(workspace(&dir))).await,
            Err(ExecError::InvalidCwd { .. })
        ));
    }

    #[tokio::test]
    async fn rejects_a_relative_cwd_in_direct_mode() {
        let request = ExecRequest {
            cwd: Some("relative".to_owned()),
            ..request("true", &[])
        };

        assert!(matches!(
            exec(request, None).await,
            Err(ExecError::InvalidCwd { .. })
        ));
    }

    #[tokio::test]
    async fn rejects_an_empty_command() {
        assert!(matches!(
            exec(request("  ", &[]), None).await,
            Err(ExecError::EmptyCommand)
        ));
    }

    #[tokio::test]
    async fn reports_a_missing_program_as_a_spawn_error() {
        assert!(matches!(
            exec(request("definitely-not-a-real-command", &[]), None).await,
            Err(ExecError::Spawn { .. })
        ));
    }

    fn shell_request(script: &str) -> ShellRequest {
        ShellRequest {
            script: script.to_owned(),
            cwd: None,
            env: HashMap::new(),
            shell: None,
            timeout: None,
        }
    }

    /// Run a shell request expected to spawn, collecting its whole response.
    async fn run_shell(request: ShellRequest) -> Vec<Frame> {
        shell(request, None).await.unwrap().collect().await
    }

    #[tokio::test]
    async fn shell_runs_a_script_through_the_default_interpreter() {
        let frames = run_shell(shell_request("printf out; printf err >&2; exit 4")).await;

        assert_eq!(stream_bytes(&frames, false), b"out");
        assert_eq!(stream_bytes(&frames, true), b"err");
        assert_eq!(final_status(&frames), Status::Exited { code: 4 });
    }

    #[tokio::test]
    async fn reports_an_unknown_shell_as_a_spawn_error() {
        let request = ShellRequest {
            shell: Some("definitely-not-a-real-shell".to_owned()),
            ..shell_request("true")
        };

        assert!(matches!(
            shell(request, None).await,
            Err(ExecError::Spawn { .. })
        ));
    }

    #[tokio::test]
    async fn shell_shares_the_workspace_boundary() {
        let dir = TempDir::new();
        let request = ShellRequest {
            cwd: Some("../outside".to_owned()),
            ..shell_request("true")
        };

        assert!(matches!(
            shell(request, Some(workspace(&dir))).await,
            Err(ExecError::InvalidCwd { .. })
        ));
    }
}
