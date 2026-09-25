//! The exec, shell and pty HTTP endpoints.
//!
//! exec and shell both answer in one of two shapes, chosen by the server the way the MCP Streamable
//! HTTP transport does: a command that finishes within the request's `timeout` —
//! [`DIRECT_RESPONSE_TIMEOUT`] when it names none — returns a single [`ExecResult`],
//! while one that runs longer returns the multiplexed frame stream of [`exec`], keeping
//! stdout, stderr and the terminal status apart. The probe buffers at most
//! [`DIRECT_RESPONSE_LIMIT`] bytes, so a command that produces output faster than it exits
//! cannot force unbounded memory.
//!
//! pty is addressed in two steps, because its session outlives one request: `POST .../pty`
//! creates a session and answers with the WebSocket endpoint of [`PtySession`], which
//! `GET .../pty/{session_id}` then attaches to. The connection speaks the frames of
//! [`super::pty`], not the exec stream.

use std::collections::HashMap;
use std::convert::Infallible;
use std::time::{Duration, Instant};

use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::ws::{WebSocket, WebSocketUpgrade};
use axum::extract::{FromRequestParts, Path};
use axum::http::header;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use axum::routing;
use bytes::Bytes;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;

use super::exec::{self, ExecError, Frame};
use super::model::{ExecRequest, ExecResult, PtyRequest, PtySession, ShellRequest};
use crate::workspace::registry::WorkspaceEnvironment;
use crate::{AppError, AppState};

/// How long a command may run before its response is upgraded to a stream, when the
/// request names no `timeout`.
const DIRECT_RESPONSE_TIMEOUT: Duration = Duration::from_millis(500);

/// Most output the probe buffers before it gives up on a direct response.
const DIRECT_RESPONSE_LIMIT: usize = 1024 * 1024;

/// Media type of the upgraded response: the length-prefixed frames of [`exec`].
const STREAM_CONTENT_TYPE: &str = "application/vnd.sandbox-toolkit.exec-stream";

/// Build the process router.
pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/exec", routing::post(exec_endpoint))
        .route(
            "/workspaces/{workspace_id}/exec",
            routing::post(exec_endpoint),
        )
        .route("/shell", routing::post(shell_endpoint))
        .route(
            "/workspaces/{workspace_id}/shell",
            routing::post(shell_endpoint),
        )
        .route("/pty", routing::post(create_pty))
        .route("/workspaces/{workspace_id}/pty", routing::post(create_pty))
        .route("/pty/{session_id}", routing::get(attach_pty))
        .route(
            "/workspaces/{workspace_id}/pty/{session_id}",
            routing::get(attach_pty),
        )
}

/// The workspace a process route runs in, resolved from the path.
///
/// `None` is direct mode, where `cwd` is a remote absolute path.
struct ProcessTarget(Option<WorkspaceEnvironment>);

impl FromRequestParts<AppState> for ProcessTarget {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let Path(captures): Path<HashMap<String, String>> = Path::from_request_parts(parts, state)
            .await
            .map_err(|_| AppError::BadRequest("invalid process path".to_owned()))?;

        match captures.get("workspace_id") {
            Some(id) => Ok(Self(Some(state.workspaces().environment(id)?))),
            None => Ok(Self(None)),
        }
    }
}

async fn exec_endpoint(
    target: ProcessTarget,
    Json(request): Json<ExecRequest>,
) -> Result<Response, AppError> {
    let timeout = probe_timeout(request.timeout);
    let stream = exec::exec(request, target.0).await?;

    respond(stream, timeout).await
}

async fn shell_endpoint(
    target: ProcessTarget,
    Json(request): Json<ShellRequest>,
) -> Result<Response, AppError> {
    let timeout = probe_timeout(request.timeout);
    let stream = exec::shell(request, target.0).await?;

    respond(stream, timeout).await
}

/// Create a pty session and answer with the endpoint that attaches to it.
async fn create_pty(
    _target: ProcessTarget,
    Json(_request): Json<PtyRequest>,
) -> Result<Json<PtySession>, AppError> {
    Err(AppError::NotImplemented("POST pty"))
}

/// Attach to an existing pty session over a WebSocket.
///
/// The upgrade carries the session's frames, so the response leaves the HTTP error
/// envelope: a rejection before the upgrade, such as a request that is not a WebSocket
/// handshake, is answered by axum with its plain text body.
async fn attach_pty(
    _target: ProcessTarget,
    Path(_captures): Path<HashMap<String, String>>,
    upgrade: WebSocketUpgrade,
) -> Result<Response, AppError> {
    Ok(upgrade.on_upgrade(session_socket))
}

/// Drive one pty session over `socket`, speaking the frames of [`super::pty`].
async fn session_socket(_socket: WebSocket) {}

/// Map a failed command onto the shared HTTP error envelope.
///
/// Every variant is the caller's doing: an empty command, an unusable `cwd`, or an
/// executable that does not exist.
impl From<ExecError> for AppError {
    fn from(error: ExecError) -> Self {
        Self::BadRequest(error.to_string())
    }
}

/// The probe deadline of a request: its `timeout` in milliseconds, or
/// [`DIRECT_RESPONSE_TIMEOUT`] when it names none.
fn probe_timeout(request_timeout: Option<u64>) -> Duration {
    request_timeout.map_or(DIRECT_RESPONSE_TIMEOUT, Duration::from_millis)
}

/// Answer `stream` with a direct result or the stream itself, waiting up to `timeout`.
async fn respond(stream: ReceiverStream<Frame>, timeout: Duration) -> Result<Response, AppError> {
    respond_with(stream, timeout, DIRECT_RESPONSE_LIMIT).await
}

/// Wait up to `timeout` for the terminal status, buffering at most `limit` bytes.
///
/// Reaching either bound switches to the stream and flushes what was buffered, so
/// no frame is lost across the two response shapes.
async fn respond_with(
    mut stream: ReceiverStream<Frame>,
    timeout: Duration,
    limit: usize,
) -> Result<Response, AppError> {
    let deadline = Instant::now() + timeout;
    let mut buffered = Vec::new();
    let mut buffered_bytes = 0;

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());

        match tokio::time::timeout(remaining, stream.next()).await {
            Ok(Some(frame)) => {
                buffered_bytes += frame.payload_len();
                let complete = matches!(frame, Frame::Status(_));
                buffered.push(frame);

                if complete {
                    return direct(&buffered);
                }
                if buffered_bytes > limit {
                    return Ok(streaming(buffered, stream));
                }
            }
            // The pump always ends the stream with a status, so its absence is a bug
            // rather than a client error.
            Ok(None) => return Err(missing_status()),
            Err(_) => return Ok(streaming(buffered, stream)),
        }
    }
}

/// Collapse a finished stream into one result.
fn direct(frames: &[Frame]) -> Result<Response, AppError> {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut status = None;

    for frame in frames {
        match frame {
            Frame::Stdout(bytes) => stdout.extend_from_slice(bytes),
            Frame::Stderr(bytes) => stderr.extend_from_slice(bytes),
            Frame::Status(value) => status = Some(value.clone()),
        }
    }

    let result = ExecResult {
        status: status.ok_or_else(missing_status)?,
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    };

    Ok(Json(result).into_response())
}

/// The frames already read, followed by the rest of the command's output.
fn streaming(head: Vec<Frame>, rest: ReceiverStream<Frame>) -> Response {
    let body = tokio_stream::iter(head)
        .chain(rest)
        .map(|frame| Ok::<Bytes, Infallible>(frame.encode()));

    Response::builder()
        .header(header::CONTENT_TYPE, STREAM_CONTENT_TYPE)
        .body(Body::from_stream(body))
        .expect("a response built from one valid header and a body is valid")
}

fn missing_status() -> AppError {
    AppError::Internal(anyhow::anyhow!(
        "the command stream ended without a terminal status"
    ))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::*;
    use crate::process::exec::FrameStream;
    use crate::process::model::Status;
    use crate::workspace::model::WorkspaceProperties;
    use crate::workspace::registry::test_support::{TempDir, canonical};

    /// The router with a fresh state, ready to drive through `oneshot`.
    fn app() -> Router {
        router().with_state(AppState::new("."))
    }

    fn json_request(method: &str, uri: &str, body: &Value) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    async fn send(app: &Router, request: Request<Body>) -> Response {
        app.clone().oneshot(request).await.unwrap()
    }

    async fn json_body(response: Response) -> Value {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();

        serde_json::from_slice(&bytes).unwrap()
    }

    /// Decode a streamed body into frames, stopping at the terminal status.
    async fn frames(response: Response) -> Vec<Frame> {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let mut stream = FrameStream::default();
        stream.feed(&bytes);

        let mut collected = Vec::new();
        while let Some(frame) = stream.decode().unwrap() {
            let complete = matches!(frame, Frame::Status(_));
            collected.push(frame);
            if complete {
                break;
            }
        }

        collected
    }

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

    fn final_status(frames: &[Frame]) -> Status {
        match frames.last() {
            Some(Frame::Status(status)) => status.clone(),
            other => panic!("the stream did not end with a status: {other:?}"),
        }
    }

    #[tokio::test]
    async fn exec_answers_with_a_direct_result() {
        let response = send(
            &app(),
            json_request(
                "POST",
                "/exec",
                &json!({ "command": "sh", "args": ["-c", "printf out; printf err >&2; exit 3"] }),
            ),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE].to_str().unwrap(),
            "application/json"
        );
        assert_eq!(
            json_body(response).await,
            json!({
                "status": { "status": "exited", "code": 3 },
                "stdout": "out",
                "stderr": "err",
            })
        );
    }

    #[tokio::test]
    async fn shell_answers_with_a_direct_result() {
        let response = send(
            &app(),
            json_request("POST", "/shell", &json!({ "script": "printf hi" })),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(json_body(response).await["stdout"], json!("hi"));
    }

    #[tokio::test]
    async fn exec_resolves_the_workspace_and_injects_its_variable() {
        let dir = TempDir::new();
        let state = AppState::new(".");
        state
            .workspaces()
            .register("docs", &dir.root(), WorkspaceProperties::default())
            .await
            .unwrap();
        let app = router().with_state(state);

        let response = send(
            &app,
            json_request(
                "POST",
                "/workspaces/docs/exec",
                &json!({
                    "command": "sh",
                    "args": ["-c", "printf %s \"$WORKSPACE_DOCS\""],
                    "cwd": ".",
                }),
            ),
        )
        .await;

        assert_eq!(
            json_body(response).await["stdout"],
            json!(canonical(dir.path()).to_str().unwrap())
        );
    }

    #[tokio::test]
    async fn unknown_workspaces_are_reported() {
        let response = send(
            &app(),
            json_request(
                "POST",
                "/workspaces/missing/exec",
                &json!({ "command": "true" }),
            ),
        )
        .await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            json_body(response).await["error"]["code"],
            json!("not_found")
        );
    }

    #[tokio::test]
    async fn invalid_requests_are_rejected() {
        let app = app();

        for body in [
            json!({ "command": "  " }),
            json!({ "command": "true", "cwd": "relative" }),
            json!({ "command": "definitely-not-a-real-command" }),
        ] {
            let response = send(&app, json_request("POST", "/exec", &body)).await;

            assert_eq!(
                response.status(),
                StatusCode::BAD_REQUEST,
                "unexpected status for {body}"
            );
            assert_eq!(
                json_body(response).await["error"]["code"],
                json!("bad_request")
            );
        }
    }

    #[tokio::test]
    async fn a_probe_that_expires_upgrades_to_the_frame_stream() {
        let request = ExecRequest {
            command: "sh".to_owned(),
            args: vec![
                "-c".to_owned(),
                "printf out; printf err >&2; exit 3".to_owned(),
            ],
            cwd: None,
            env: HashMap::new(),
            timeout: None,
        };
        let stream = exec::exec(request, None).await.unwrap();

        let response = respond_with(stream, Duration::ZERO, DIRECT_RESPONSE_LIMIT)
            .await
            .unwrap();

        assert_eq!(
            response.headers()[header::CONTENT_TYPE].to_str().unwrap(),
            STREAM_CONTENT_TYPE
        );

        let frames = frames(response).await;
        assert_eq!(stream_bytes(&frames, false), b"out");
        assert_eq!(stream_bytes(&frames, true), b"err");
        assert_eq!(final_status(&frames), Status::Exited { code: 3 });
    }

    #[tokio::test]
    async fn exec_streams_a_command_that_outlives_the_probe() {
        let response = send(
            &app(),
            json_request(
                "POST",
                "/exec",
                &json!({ "command": "sh", "args": ["-c", "sleep 1; printf late"] }),
            ),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE].to_str().unwrap(),
            STREAM_CONTENT_TYPE
        );

        let frames = frames(response).await;
        assert_eq!(stream_bytes(&frames, false), b"late");
        assert_eq!(final_status(&frames), Status::Success);
    }

    #[tokio::test]
    async fn a_short_request_timeout_upgrades_the_response() {
        let response = send(
            &app(),
            json_request(
                "POST",
                "/exec",
                &json!({
                    "command": "sh",
                    "args": ["-c", "sleep 0.3; printf late"],
                    "timeout": 50,
                }),
            ),
        )
        .await;

        assert_eq!(
            response.headers()[header::CONTENT_TYPE].to_str().unwrap(),
            STREAM_CONTENT_TYPE
        );

        let frames = frames(response).await;
        assert_eq!(stream_bytes(&frames, false), b"late");
    }

    #[tokio::test]
    async fn a_long_request_timeout_keeps_a_slow_command_direct() {
        let response = send(
            &app(),
            json_request(
                "POST",
                "/exec",
                &json!({
                    "command": "sh",
                    "args": ["-c", "sleep 0.3; printf late"],
                    "timeout": 5000,
                }),
            ),
        )
        .await;

        assert_eq!(
            response.headers()[header::CONTENT_TYPE].to_str().unwrap(),
            "application/json"
        );
        assert_eq!(json_body(response).await["stdout"], json!("late"));
    }

    #[tokio::test]
    async fn shell_honors_the_request_timeout() {
        let response = send(
            &app(),
            json_request(
                "POST",
                "/shell",
                &json!({ "script": "sleep 0.3; printf late", "timeout": 50 }),
            ),
        )
        .await;

        assert_eq!(
            response.headers()[header::CONTENT_TYPE].to_str().unwrap(),
            STREAM_CONTENT_TYPE
        );

        let frames = frames(response).await;
        assert_eq!(stream_bytes(&frames, false), b"late");
    }

    #[tokio::test]
    async fn pty_routes_are_wired() {
        let app = app();

        let create = send(
            &app,
            json_request("POST", "/pty", &json!({ "command": "sh" })),
        )
        .await;
        assert_eq!(create.status(), StatusCode::NOT_IMPLEMENTED);
        assert_eq!(
            json_body(create).await["error"]["code"],
            json!("not_implemented")
        );

        // The attach route is a WebSocket endpoint, so a request that is not a
        // handshake is rejected before any session is reached.
        let attach = send(
            &app,
            Request::builder()
                .method("GET")
                .uri("/pty/session-1")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(attach.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn pty_rejects_an_unknown_workspace_before_creating_a_session() {
        let response = send(
            &app(),
            json_request(
                "POST",
                "/workspaces/missing/pty",
                &json!({ "command": "sh" }),
            ),
        )
        .await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            json_body(response).await["error"]["code"],
            json!("not_found")
        );
    }
}
