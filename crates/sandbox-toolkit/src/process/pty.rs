//! A pty session runs over a WebSocket, so every message already carries its own
//! boundary and a frame needs no length prefix.
//!
//! The channel set is closed, so a connection never creates channels. It has no
//! stderr because a pty folds stderr into stdout.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use axum::extract::ws::CloseFrame;
use axum::extract::ws::Message;
use axum::extract::ws::Utf8Bytes;
use axum::extract::ws::WebSocket;
use bytes::{BufMut, Bytes, BytesMut};
use pty::ProcessExit;
use pty::ProcessHandle;
use pty::SpawnedProcess;
use pty::TerminalSize as PtyTerminalSize;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::time::Instant;
use ts_rs::TS;

use super::exec::CommandSpec;
use super::exec::ExecError;
use super::exec::WorkspaceContext;
use super::model::PtyRequest;
use super::model::PtySession;
use super::model::Status;

/// The largest WebSocket message a session accepts, matching the exec frame
/// payload bound.
pub(crate) const MAX_MESSAGE_LEN: usize = 4 * 1024 * 1024;

/// A session nobody connects to within this window is reaped.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// After the connection is established, this much silence (counting ping/pong)
/// reaps the session.
const IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// The discriminants are wire identifiers; reordering them changes the format.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Channel {
    Stdin = 0,
    /// With stderr folded in.
    Stdout = 1,
    /// The process outcome, carrying the exit code or an error.
    Exit = 3,
    Resize = 4,
}

impl Channel {
    pub(crate) const fn as_u8(self) -> u8 {
        self as u8
    }

    pub(crate) fn from_u8(id: u8) -> Result<Self, FrameError> {
        match id {
            0 => Ok(Self::Stdin),
            1 => Ok(Self::Stdout),
            3 => Ok(Self::Exit),
            4 => Ok(Self::Resize),
            other => Err(FrameError::UnknownChannel(other)),
        }
    }

    pub(crate) const fn direction(self) -> Direction {
        match self {
            Self::Stdin | Self::Resize => Direction::ClientToServer,
            Self::Stdout | Self::Exit => Direction::ServerToClient,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    ClientToServer,
    ServerToClient,
}

/// Terminal geometry in character cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub(crate) struct TerminalSize {
    pub(crate) rows: u16,
    pub(crate) cols: u16,
}

impl Default for TerminalSize {
    fn default() -> Self {
        Self {
            rows: 24,
            cols: 80,
        }
    }
}

impl TerminalSize {
    pub(crate) const WIRE_LEN: usize = 4;

    pub(crate) fn to_wire(self) -> [u8; Self::WIRE_LEN] {
        let mut wire = [0u8; Self::WIRE_LEN];
        wire[..2].copy_from_slice(&self.rows.to_be_bytes());
        wire[2..].copy_from_slice(&self.cols.to_be_bytes());
        wire
    }

    /// Reads the leading [`Self::WIRE_LEN`] bytes and ignores any trailing bytes, so
    /// a later, larger encoding of the size stays decodable.
    pub(crate) fn from_wire(payload: &[u8]) -> Option<Self> {
        let [rows_hi, rows_lo, cols_hi, cols_lo, ..] = payload else {
            return None;
        };

        Some(Self {
            rows: u16::from_be_bytes([*rows_hi, *rows_lo]),
            cols: u16::from_be_bytes([*cols_hi, *cols_lo]),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Frame {
    Stdin(Bytes),
    Stdout(Bytes),
    /// Ends the output stream.
    Exit(Status),
    Resize(TerminalSize),
}

impl Frame {
    pub(crate) const fn channel(&self) -> Channel {
        match self {
            Self::Stdin(_) => Channel::Stdin,
            Self::Stdout(_) => Channel::Stdout,
            Self::Exit(_) => Channel::Exit,
            Self::Resize(_) => Channel::Resize,
        }
    }

    pub(crate) const fn direction(&self) -> Direction {
        self.channel().direction()
    }

    pub(crate) fn encode(&self) -> Bytes {
        let payload = self.payload();
        let mut message = BytesMut::with_capacity(1 + payload.len());
        message.put_u8(self.channel().as_u8());
        message.put_slice(&payload);
        message.freeze()
    }

    pub(crate) fn decode(message: &[u8]) -> Result<Self, FrameError> {
        let Some((&id, payload)) = message.split_first() else {
            return Err(FrameError::EmptyMessage);
        };

        Self::from_payload(Channel::from_u8(id)?, payload)
    }

    fn payload(&self) -> Bytes {
        match self {
            Self::Stdin(bytes) | Self::Stdout(bytes) => bytes.clone(),
            Self::Exit(status) => serde_json::to_vec(status)
                .expect("a status frame always serializes to JSON")
                .into(),
            Self::Resize(size) => Bytes::copy_from_slice(&size.to_wire()),
        }
    }

    fn from_payload(channel: Channel, payload: &[u8]) -> Result<Self, FrameError> {
        let frame = match channel {
            Channel::Stdin => Self::Stdin(Bytes::copy_from_slice(payload)),
            Channel::Stdout => Self::Stdout(Bytes::copy_from_slice(payload)),
            Channel::Exit => {
                Self::Exit(serde_json::from_slice(payload).map_err(FrameError::InvalidStatus)?)
            }
            Channel::Resize => Self::Resize(TerminalSize::from_wire(payload).ok_or(
                FrameError::InvalidPayload {
                    channel,
                    expected: TerminalSize::WIRE_LEN,
                    length: payload.len(),
                },
            )?),
        };

        Ok(frame)
    }
}

/// [`Frame::Exit`] is terminal for output: nothing follows it.
#[derive(Debug, Default)]
#[allow(
    dead_code,
    reason = "the server-to-client sequence is produced directly by the session runtime; \
              this validator is exercised only by tests"
)]
pub(crate) struct SessionFrames {
    exited: bool,
}

#[allow(
    dead_code,
    reason = "exercised only by the session-frame validator tests"
)]
impl SessionFrames {
    pub(crate) fn decode(&mut self, message: &[u8]) -> Result<Frame, FrameError> {
        if self.exited {
            return Err(FrameError::FrameAfterExit);
        }

        let frame = Frame::decode(message)?;
        if matches!(frame, Frame::Exit(_)) {
            self.exited = true;
        }

        Ok(frame)
    }

    pub(crate) const fn exited(&self) -> bool {
        self.exited
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum FrameError {
    #[error("empty WebSocket message carries no channel byte")]
    EmptyMessage,
    #[error("unknown channel id: {0}")]
    UnknownChannel(u8),
    #[error("channel {channel:?} payload must be at least {expected} bytes, got {length}")]
    InvalidPayload {
        channel: Channel,
        expected: usize,
        length: usize,
    },
    #[error("exit frame payload is not valid JSON")]
    InvalidStatus(#[source] serde_json::Error),
    #[allow(
        dead_code,
        reason = "constructed only by the session-frame validator, which is test-only"
    )]
    #[error("frame received after the terminal exit frame")]
    FrameAfterExit,
}

/// Live pty sessions, keyed by id. Attaching removes a session, so exactly one
/// connection can own a terminal and the creation-timeout reaper cannot race an
/// attach.
#[derive(Clone, Debug, Default)]
pub(crate) struct PtySessions {
    sessions: Arc<StdMutex<HashMap<String, Arc<Session>>>>,
}

impl PtySessions {
    fn insert(&self, session: Arc<Session>) {
        if let Ok(mut sessions) = self.sessions.lock() {
            sessions.insert(session.id.clone(), session);
        }
    }

    /// Removes the session, handing ownership to the one caller that attaches.
    pub(crate) fn take(&self, id: &str) -> Option<Arc<Session>> {
        self.sessions.lock().ok()?.remove(id)
    }

    /// Ends the creation grace period. A session that was attached is already
    /// gone from the map, so this only terminates one still awaiting a connection.
    fn expire(&self, id: &str) {
        let expired = self
            .sessions
            .lock()
            .ok()
            .and_then(|mut sessions| sessions.remove(id));

        if let Some(session) = expired {
            session.handle.terminate();
        }
    }
}

/// A spawned pty process plus the channels its connection drains.
#[derive(Debug)]
pub(crate) struct Session {
    id: String,
    workspace_id: Option<String>,
    handle: Arc<ProcessHandle>,
    // Taken by [`Self::run`] on attach, so a session can never be attached twice.
    stdout_rx: StdMutex<Option<mpsc::Receiver<Vec<u8>>>>,
    exit_rx: StdMutex<Option<oneshot::Receiver<ProcessExit>>>,
}

impl Session {
    pub(crate) fn workspace_id(&self) -> Option<&str> {
        self.workspace_id.as_deref()
    }

    /// Terminates the process group, used when an attach is rejected after the
    /// session was taken.
    pub(crate) fn terminate(&self) {
        self.handle.terminate();
    }

    /// Drives one attached WebSocket to completion: client stdin and resize in,
    /// pty stdout out, then the terminal status and a close. Every exit path
    /// terminates the process group, so no descendant outlives the connection.
    pub(crate) async fn run(self: Arc<Self>, mut socket: WebSocket) {
        let (Some(mut stdout_rx), Some(mut exit_rx)) = (
            self.stdout_rx.lock().ok().and_then(|mut rx| rx.take()),
            self.exit_rx.lock().ok().and_then(|mut rx| rx.take()),
        ) else {
            return;
        };

        let writer = self.handle.writer_sender();
        let mut last_activity = Instant::now();
        let mut status = None;
        let mut stdout_open = true;
        let mut exit_open = true;

        loop {
            // Both directions are finished: the exit status is known and the pty
            // has no more output.
            if !stdout_open && !exit_open {
                break;
            }

            // Reading the socket and writing frames must not share a borrow, so
            // the select only produces an event; the socket is used afterwards.
            let idle_deadline = last_activity + IDLE_TIMEOUT;
            let event = tokio::select! {
                incoming = socket.recv() => ClientEvent::Message(incoming),
                chunk = stdout_rx.recv(), if stdout_open => ClientEvent::Stdout(chunk),
                result = &mut exit_rx, if exit_open => ClientEvent::Exit(result),
                () = tokio::time::sleep_until(idle_deadline) => ClientEvent::Idle,
            };

            match event {
                ClientEvent::Message(Some(Ok(Message::Binary(message)))) => {
                    last_activity = Instant::now();
                    let frame = match Frame::decode(&message) {
                        Ok(frame) => frame,
                        Err(_) => break,
                    };

                    match frame.direction() {
                        Direction::ClientToServer => match frame {
                            Frame::Stdin(bytes) => {
                                let _ = writer.send(bytes.to_vec()).await;
                            }
                            Frame::Resize(size) => {
                                let _ = self.handle.resize(PtyTerminalSize {
                                    rows: size.rows,
                                    cols: size.cols,
                                });
                            }
                            // The direction is fixed by the channel, so this is
                            // unreachable; closing keeps the connection bounded.
                            _ => break,
                        },
                        // A server-only channel is a protocol error that ends the
                        // session.
                        Direction::ServerToClient => break,
                    }
                }
                // Ping is answered by the transport; both count as activity.
                ClientEvent::Message(Some(Ok(Message::Ping(_) | Message::Pong(_)))) => {
                    last_activity = Instant::now();
                }
                ClientEvent::Message(Some(Ok(Message::Close(_)))) | ClientEvent::Idle => break,
                ClientEvent::Message(Some(Ok(Message::Text(_)))) => {}
                ClientEvent::Message(Some(Err(_)) | None) => break,
                ClientEvent::Stdout(Some(bytes)) => {
                    let frame = Frame::Stdout(Bytes::from(bytes));
                    if socket.send(Message::Binary(frame.encode())).await.is_err() {
                        break;
                    }
                }
                ClientEvent::Stdout(None) => stdout_open = false,
                ClientEvent::Exit(result) => {
                    status = Some(result.unwrap_or_else(|_| ProcessExit::exited(-1)));
                    exit_open = false;
                }
            }
        }

        if let Some(status) = status {
            let frame = Frame::Exit(status_from_exit(&status));
            let _ = socket.send(Message::Binary(frame.encode())).await;
        }
        let _ = socket
            .send(Message::Close(Some(CloseFrame {
                code: 1000,
                reason: Utf8Bytes::from_static("session ended"),
            })))
            .await;

        self.handle.terminate();
    }
}

/// What one `select!` iteration produced, kept separate so the socket is never
/// borrowed for both reading and writing in the same expression.
enum ClientEvent {
    Message(Option<Result<Message, axum::Error>>),
    Stdout(Option<Vec<u8>>),
    Exit(Result<ProcessExit, oneshot::error::RecvError>),
    Idle,
}

/// Resolves the command like exec does, spawns it under a pty and registers the
/// session for a later attach.
pub(crate) async fn create(
    sessions: &PtySessions,
    request: PtyRequest,
    context: WorkspaceContext,
    workspace_id: Option<String>,
) -> Result<PtySession, ExecError> {
    let PtyRequest {
        command,
        args,
        cwd,
        env,
        size,
    } = request;

    if command.trim().is_empty() {
        return Err(ExecError::EmptyCommand);
    }

    let spec = CommandSpec::resolve(
        command,
        args,
        cwd,
        env,
        context.workspace.as_ref(),
        context.environment,
    )
    .await?;

    let size = size.unwrap_or_default();
    let environment = spec.environment();
    let spawned = pty::spawn_pty_process(
        &spec.program,
        &spec.args,
        &spec.cwd,
        &environment,
        &None,
        PtyTerminalSize {
            rows: size.rows,
            cols: size.cols,
        },
    )
    .await
    .map_err(|error| ExecError::PtySpawn {
        command: spec.program.clone(),
        message: error.to_string(),
    })?;

    let SpawnedProcess {
        session,
        stdout_rx,
        // The pty folds stderr into stdout, so only stdout crosses the socket.
        stderr_rx: _,
        exit_rx,
    } = spawned;

    let id = new_session_id();
    let endpoint = match &workspace_id {
        Some(workspace_id) => format!("/workspaces/{workspace_id}/pty/{id}"),
        None => format!("/pty/{id}"),
    };

    let session = Arc::new(Session {
        id: id.clone(),
        workspace_id,
        handle: Arc::new(session),
        stdout_rx: StdMutex::new(Some(stdout_rx)),
        exit_rx: StdMutex::new(Some(exit_rx)),
    });

    sessions.insert(Arc::clone(&session));

    let expiry_sessions = sessions.clone();
    let expiry_id = id.clone();
    tokio::spawn(async move {
        tokio::time::sleep(CONNECT_TIMEOUT).await;
        expiry_sessions.expire(&expiry_id);
    });

    Ok(PtySession { id, endpoint })
}

/// A signal-terminated process reports `failed`, so a client distinguishes it
/// from a real exit code.
fn status_from_exit(exit: &ProcessExit) -> Status {
    match &exit.signal {
        Some(signal) => Status::Failed {
            message: format!("terminated by {signal}"),
        },
        None => Status::Exited {
            exit_code: exit.exit_code,
        },
    }
}

/// Collision-resistant within a process, and unguessable enough that a session
/// id is not an access token on its own.
fn new_session_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let seed = format!("{}:{counter}:{nanos}", std::process::id());
    let digest = blake3::hash(seed.as_bytes());

    format!("pty_{}", &digest.to_hex()[..32])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples() -> Vec<Frame> {
        vec![
            Frame::Stdin(Bytes::from_static(b"in")),
            Frame::Stdout(Bytes::from_static(b"out")),
            Frame::Exit(Status::Exited { exit_code: 3 }),
            Frame::Resize(TerminalSize { rows: 24, cols: 80 }),
        ]
    }

    #[test]
    fn channel_ids_are_wire_stable() {
        assert_eq!(Channel::Stdin.as_u8(), 0);
        assert_eq!(Channel::Stdout.as_u8(), 1);
        assert_eq!(Channel::Exit.as_u8(), 3);
        assert_eq!(Channel::Resize.as_u8(), 4);
    }

    #[test]
    fn channels_carry_their_direction() {
        assert_eq!(Channel::Stdin.direction(), Direction::ClientToServer);
        assert_eq!(Channel::Resize.direction(), Direction::ClientToServer);
        assert_eq!(Channel::Stdout.direction(), Direction::ServerToClient);
        assert_eq!(Channel::Exit.direction(), Direction::ServerToClient);
    }

    #[test]
    fn round_trips_every_frame() {
        for frame in samples() {
            assert_eq!(Frame::decode(&frame.encode()).unwrap(), frame);
        }
    }

    #[test]
    fn round_trips_every_status() {
        let statuses = [
            Status::Exited { exit_code: 0 },
            Status::Exited { exit_code: 1 },
            Status::Failed {
                message: "spawn failed".to_owned(),
            },
        ];

        for status in statuses {
            let frame = Frame::Exit(status);
            assert_eq!(Frame::decode(&frame.encode()).unwrap(), frame);
        }
    }

    #[test]
    fn a_message_is_a_whole_frame() {
        assert_eq!(
            Frame::Stdin(Bytes::from_static(b"hi")).encode().as_ref(),
            b"\x00hi"
        );
    }

    #[test]
    fn exit_payload_is_json() {
        let status = Status::Exited { exit_code: 2 };
        let encoded = Frame::Exit(status.clone()).encode();

        assert_eq!(
            encoded.slice(1..).as_ref(),
            serde_json::to_vec(&status).unwrap()
        );
    }

    #[test]
    fn rejects_an_empty_message() {
        assert!(matches!(Frame::decode(&[]), Err(FrameError::EmptyMessage)));
    }

    #[test]
    fn rejects_a_stderr_channel() {
        // A pty folds stderr into stdout, so id 2 is not a channel.
        assert!(matches!(
            Frame::decode(&[2]),
            Err(FrameError::UnknownChannel(2))
        ));
    }

    #[test]
    fn rejects_an_unknown_channel() {
        // 255 was the in-band close channel; closing is the WebSocket close frame.
        for id in [5, 255] {
            assert!(matches!(
                Frame::decode(&[id]),
                Err(FrameError::UnknownChannel(unknown)) if unknown == id
            ));
        }
    }

    #[test]
    fn rejects_a_resize_payload_shorter_than_two_integers() {
        assert!(matches!(
            Frame::decode(&[Channel::Resize.as_u8(), 0, 0, 0]),
            Err(FrameError::InvalidPayload {
                channel: Channel::Resize,
                expected: 4,
                length: 3,
            })
        ));
    }

    #[test]
    fn accepts_a_resize_payload_with_trailing_bytes() {
        // The size is a minimum, so a later, larger encoding still decodes.
        let frame = Frame::decode(&[Channel::Resize.as_u8(), 0, 24, 0, 80, 9, 9]).unwrap();

        assert_eq!(frame, Frame::Resize(TerminalSize { rows: 24, cols: 80 }));
    }

    #[test]
    fn rejects_a_malformed_exit_payload() {
        assert!(matches!(
            Frame::decode(&[Channel::Exit.as_u8(), b'n', b'/', b'a']),
            Err(FrameError::InvalidStatus(_))
        ));
    }

    #[test]
    fn nothing_follows_the_exit_frame() {
        let mut frames = SessionFrames::default();
        frames
            .decode(&Frame::Exit(Status::Exited { exit_code: 0 }).encode())
            .unwrap();
        assert!(frames.exited());

        assert!(matches!(
            frames.decode(&Frame::Stdout(Bytes::from_static(b"late")).encode()),
            Err(FrameError::FrameAfterExit)
        ));
        assert!(matches!(
            frames.decode(&Frame::Exit(Status::Exited { exit_code: 0 }).encode()),
            Err(FrameError::FrameAfterExit)
        ));
    }

    fn request(command: &str, args: &[&str], cwd: Option<&str>) -> PtyRequest {
        PtyRequest {
            command: command.to_owned(),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            cwd: cwd.map(str::to_owned),
            env: HashMap::new(),
            size: None,
        }
    }

    #[tokio::test]
    async fn create_spawns_and_registers_a_one_shot_session() {
        let sessions = PtySessions::default();
        let dto = create(
            &sessions,
            request("sh", &["-c", "true"], None),
            WorkspaceContext::default(),
            None,
        )
        .await
        .unwrap();

        assert!(dto.id.starts_with("pty_"), "unexpected id: {}", dto.id);
        assert_eq!(dto.endpoint, format!("/pty/{}", dto.id));

        // Attaching removes the session, so it cannot be attached twice.
        let session = sessions.take(&dto.id).expect("the session is registered");
        assert!(sessions.take(&dto.id).is_none());

        session.terminate();
    }

    #[tokio::test]
    async fn create_scopes_the_endpoint_to_the_workspace() {
        let sessions = PtySessions::default();
        let dto = create(
            &sessions,
            request("sh", &["-c", "true"], None),
            WorkspaceContext::default(),
            Some("docs".to_owned()),
        )
        .await
        .unwrap();

        assert_eq!(dto.endpoint, format!("/workspaces/docs/pty/{}", dto.id));
        sessions.take(&dto.id).unwrap().terminate();
    }

    #[tokio::test]
    async fn create_rejects_an_empty_or_unresolvable_command() {
        let sessions = PtySessions::default();

        assert!(matches!(
            create(
                &sessions,
                request("  ", &[], None),
                WorkspaceContext::default(),
                None
            )
            .await,
            Err(ExecError::EmptyCommand)
        ));

        assert!(matches!(
            create(
                &sessions,
                request("definitely-not-a-real-command", &[], None),
                WorkspaceContext::default(),
                None
            )
            .await,
            Err(ExecError::PtySpawn { .. })
        ));

        // A relative cwd is invalid in direct mode, exactly as for exec.
        assert!(matches!(
            create(
                &sessions,
                request("true", &[], Some("relative")),
                WorkspaceContext::default(),
                None
            )
            .await,
            Err(ExecError::InvalidCwd { .. })
        ));
    }

    #[test]
    fn maps_the_terminal_outcome() {
        assert_eq!(
            status_from_exit(&ProcessExit::exited(3)),
            Status::Exited { exit_code: 3 }
        );
        assert_eq!(
            status_from_exit(&ProcessExit::signaled(143, "SIGTERM")),
            Status::Failed {
                message: "terminated by SIGTERM".to_owned()
            }
        );
    }
}
