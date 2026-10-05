use std::collections::HashMap;
use std::ffi::OsString;
use std::sync::Mutex as StdMutex;
use std::time::Duration;
use std::time::Instant;

use futures_util::SinkExt;
use futures_util::StreamExt;
use hyper::upgrade::Upgraded;
use hyper_util::rt::TokioIo;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use pty::ProcessExit;
use pty::ProcessHandle;
use pty::SpawnedProcess;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::exec::frame::Status;
use crate::pty::message;
use crate::pty::message::Client;
use crate::pty::model::PtyProcess;

/// How long a session waits for its one attach.
const PENDING: Duration = Duration::from_secs(30);

/// How long an attached session may go without WebSocket activity.
const IDLE: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Copy)]
enum Stage {
    Pending { deadline: Instant },
    Attached { deadline: Instant },
    Reclaimed,
}

pub struct Attachment {
    output: mpsc::Receiver<Vec<u8>>,
    exit: oneshot::Receiver<ProcessExit>,
}

pub struct Session {
    scope: String,
    process: ProcessHandle,
    /// Taken by the attach that claims the session.
    attachment: StdMutex<Option<Attachment>>,
    stage: StdMutex<Stage>,
    /// Fires when the session is reclaimed, so an attached client stops.
    stopped: CancellationToken,
}

impl Session {
    pub async fn spawn(scope: &str, process: &PtyProcess) -> anyhow::Result<Self> {
        let spawned = pty::spawn_pty_process(
            &process.program,
            &process.args,
            &process.cwd,
            &named(&process.env),
            /*arg0*/ &None,
            process.size,
        )
        .await?;
        // A PTY gives the child one terminal for both output streams, so the
        // stderr receiver is the closed end of a pair nobody fills.
        let SpawnedProcess {
            session: process,
            stdout_rx: output,
            exit_rx: exit,
            stderr_rx: _,
        } = spawned;

        Ok(Self {
            scope: scope.to_owned(),
            process,
            attachment: StdMutex::new(Some(Attachment { output, exit })),
            stage: StdMutex::new(Stage::Pending {
                deadline: Instant::now() + PENDING,
            }),
            stopped: CancellationToken::new(),
        })
    }

    pub fn claim(&self, scope: &str) -> Option<Attachment> {
        if self.scope != scope {
            return None;
        }

        let mut stage = self.stage.lock().ok()?;
        if !matches!(*stage, Stage::Pending { .. }) {
            return None;
        }
        *stage = Stage::Attached {
            deadline: Instant::now() + IDLE,
        };
        drop(stage);

        self.attachment.lock().ok()?.take()
    }

    fn touch(&self) {
        let Ok(mut stage) = self.stage.lock() else {
            return;
        };
        if let Stage::Attached { deadline } = &mut *stage {
            *deadline = Instant::now() + IDLE;
        }
    }

    pub fn expired(&self) -> bool {
        let Ok(stage) = self.stage.lock() else {
            return false;
        };

        match *stage {
            Stage::Pending { deadline } | Stage::Attached { deadline } => {
                Instant::now() >= deadline
            }
            Stage::Reclaimed => false,
        }
    }

    pub fn reclaimed(&self) -> bool {
        self.stage
            .lock()
            .map(|stage| matches!(*stage, Stage::Reclaimed))
            .unwrap_or(false)
    }

    pub fn reclaim(&self) {
        {
            let Ok(mut stage) = self.stage.lock() else {
                return;
            };
            if matches!(*stage, Stage::Reclaimed) {
                return;
            }
            *stage = Stage::Reclaimed;
        }

        // A session nobody attached to releases its reader here; one that is
        // attached stops through the token.
        if let Ok(mut attachment) = self.attachment.lock() {
            attachment.take();
        }
        self.stopped.cancel();
        self.process.terminate();
    }

    pub async fn run(&self, socket: WebSocketStream<TokioIo<Upgraded>>, attachment: Attachment) {
        let (mut sink, mut stream) = socket.split();
        let Attachment {
            mut output,
            mut exit,
        } = attachment;
        let mut output_ended = false;
        let mut terminal = None;

        'session: loop {
            tokio::select! {
                () = self.stopped.cancelled() => break,
                incoming = stream.next() => match incoming {
                    Some(Ok(Message::Binary(bytes))) => {
                        if !self.apply(&bytes).await {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    // Text and ping/pong carry no session traffic; they only
                    // count as activity.
                    Some(Ok(_)) => self.touch(),
                    Some(Err(_)) => break,
                },
                chunk = output.recv(), if !output_ended => match chunk {
                    Some(bytes) => {
                        for outgoing in message::stdout(&bytes) {
                            if sink.send(outgoing).await.is_err() {
                                break 'session;
                            }
                        }
                        self.touch();
                    }
                    None => output_ended = true,
                },
                // The status is the process's last word, so it follows the
                // output it belongs to.
                exit = &mut exit, if output_ended => {
                    terminal = Some(exit.unwrap_or_else(|_| ProcessExit::exited(-1)));
                    break;
                }
            }
        }

        // Leaving the socket ends the session, process group and all. Ending it
        // first keeps a client that reattaches after the close from finding it.
        self.reclaim();
        if let Some(exit) = terminal {
            let _ = sink.send(message::terminal(&status(exit))).await;
        }
        // Closing the sink puts the frame the WebSocket queued for a
        // peer-initiated close on the wire, and sends one when the session
        // ended on its own.
        let _ = sink.close().await;
    }

    async fn apply(&self, bytes: &[u8]) -> bool {
        self.touch();

        match message::decode(bytes) {
            // Input the process can no longer take is not the client's fault:
            // its end reaches the client through its output.
            Some(Client::Stdin(input)) => {
                let _ = self.process.writer_sender().send(input).await;
                true
            }
            Some(Client::Resize(size)) => {
                let _ = self.process.resize(size);
                true
            }
            None => false,
        }
    }
}

fn status(exit: ProcessExit) -> Status {
    match exit.signal {
        Some(signal) => Status::Signaled { signal },
        None => Status::Exited {
            exit_code: exit.exit_code,
        },
    }
}

/// The PTY spawn takes named variables, so an inherited entry that is not UTF-8
/// cannot survive it and is replaced.
fn named(env: &[(OsString, OsString)]) -> HashMap<String, String> {
    env.iter()
        .map(|(name, value)| {
            (
                name.to_string_lossy().into_owned(),
                value.to_string_lossy().into_owned(),
            )
        })
        .collect()
}
