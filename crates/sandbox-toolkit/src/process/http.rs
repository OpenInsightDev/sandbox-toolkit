//! exec answers with a single [`ExecResult`] when the command finishes within the
//! request's `wait`, and with the multiplexed frame stream of [`exec`] when it runs
//! longer or `wait` is zero. The probe caps what it buffers, so a command that
//! produces output faster than it exits cannot force unbounded memory.

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
use serde_json::Value;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;

use super::exec::{self, ExecError, WorkspaceContext};
use super::frame::Frame;
use super::model::{ExecRequest, ExecResult, PtyRequest, PtySession};
use crate::{AppError, AppState};

/// The most output buffered before the response upgrades to a stream; also the
/// per-frame bound, so a command that outruns its exit cannot force unbounded
/// memory.
const DIRECT_RESPONSE_LIMIT: usize = 4 * 1024 * 1024;

const STREAM_CONTENT_TYPE: &str = "application/vnd.sandbox-toolkit.exec-stream";

/// An upper bound on a control-plane JSON body, so an oversized or endless body is
/// rejected instead of buffered.
const MAX_REQUEST_BODY: usize = 64 * 1024;

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/exec", routing::post(exec_endpoint))
        .route(
            "/workspaces/{workspace_id}/exec",
            routing::post(exec_endpoint),
        )
        .route("/pty", routing::post(create_pty))
        .route("/workspaces/{workspace_id}/pty", routing::post(create_pty))
        .route("/pty/{session_id}", routing::get(attach_pty))
        .route(
            "/workspaces/{workspace_id}/pty/{session_id}",
            routing::get(attach_pty),
        )
}

struct ProcessTarget(WorkspaceContext);

impl FromRequestParts<AppState> for ProcessTarget {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let Path(captures): Path<HashMap<String, String>> = Path::from_request_parts(parts, state)
            .await
            .map_err(|_| AppError::BadRequest("invalid process path".to_owned()))?;

        let workspace = match captures.get("workspace_id") {
            Some(id) => Some(state.workspaces().workspace(id)?),
            None => None,
        };

        Ok(Self(WorkspaceContext {
            workspace,
            // Every registered workspace, injected in both addressing modes.
            environment: state.workspaces().env(),
        }))
    }
}

async fn exec_endpoint(target: ProcessTarget, body: Body) -> Result<Response, AppError> {
    let request = read_exec_request(body).await?;
    let wait = request.wait();
    let stream = exec::exec(request, target.0).await?;

    respond(stream, wait).await
}

/// Reading the body by hand keeps a malformed body on the API's own envelope. The
/// `format` tag is normalized first: absent means the default exec payload, and an
/// unknown value is a distinct error from a body that fails the chosen schema.
async fn read_exec_request(body: Body) -> Result<ExecRequest, AppError> {
    let bytes = axum::body::to_bytes(body, MAX_REQUEST_BODY)
        .await
        .map_err(|error| AppError::BadRequest(format!("failed to read request body: {error}")))?;

    let mut value: Value = serde_json::from_slice(&bytes)
        .map_err(|error| AppError::InvalidRequest(error.to_string()))?;

    let object = value
        .as_object_mut()
        .ok_or_else(|| AppError::InvalidRequest("the request body must be an object".to_owned()))?;

    match object.get("format") {
        None => {
            object.insert("format".to_owned(), Value::String("exec".to_owned()));
        }
        Some(Value::String(name)) if name == "exec" || name == "shell" => {}
        Some(Value::String(other)) => return Err(AppError::UnsupportedType(other.clone())),
        Some(_) => {
            return Err(AppError::InvalidRequest(
                "`format` must be a string".to_owned(),
            ));
        }
    }

    serde_json::from_value(value).map_err(|error| AppError::InvalidRequest(error.to_string()))
}

async fn create_pty(
    _target: ProcessTarget,
    Json(_request): Json<PtyRequest>,
) -> Result<Json<PtySession>, AppError> {
    Err(AppError::NotImplemented("POST pty"))
}

/// A rejection before the upgrade, such as a request that is not a WebSocket
/// handshake, is answered by axum with its plain text body rather than the envelope.
async fn attach_pty(
    _target: ProcessTarget,
    Path(_captures): Path<HashMap<String, String>>,
    upgrade: WebSocketUpgrade,
) -> Result<Response, AppError> {
    Ok(upgrade.on_upgrade(session_socket))
}

async fn session_socket(_socket: WebSocket) {}

/// Every variant is the caller's doing: an empty command, an unusable `cwd`, or an
/// executable that does not exist.
impl From<ExecError> for AppError {
    fn from(error: ExecError) -> Self {
        Self::BadRequest(error.to_string())
    }
}

/// A zero `wait` streams immediately, so the client deterministically gets the
/// frame stream regardless of how fast the command exits.
async fn respond(stream: ReceiverStream<Frame>, wait: u64) -> Result<Response, AppError> {
    if wait == 0 {
        return Ok(streaming(Vec::new(), stream));
    }

    respond_with(stream, Duration::from_millis(wait), DIRECT_RESPONSE_LIMIT).await
}

/// Reaching either bound switches to the stream and flushes what was buffered, so
/// no frame is lost across the two response shapes.
async fn respond_with(
    mut stream: ReceiverStream<Frame>,
    wait: Duration,
    limit: usize,
) -> Result<Response, AppError> {
    let deadline = Instant::now() + wait;
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
    use crate::process::frame::FrameStream;
    use crate::process::model::{ExecCommon, Status};
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
                &json!({
                    "command": "sh",
                    "args": ["-c", "printf out; printf err >&2; exit 3"],
                    "wait": 5000,
                }),
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
                "status": "exited",
                "exit_code": 3,
                "stdout": "out",
                "stderr": "err",
            })
        );
    }

    #[tokio::test]
    async fn shell_answers_with_a_direct_result() {
        let response = send(
            &app(),
            json_request(
                "POST",
                "/exec",
                &json!({ "format": "shell", "script": "printf hi", "wait": 5000 }),
            ),
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
                    "wait": 5000,
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
    async fn every_registered_workspace_is_injected_in_direct_mode() {
        let first = TempDir::new();
        let second = TempDir::new();
        let state = AppState::new(".");
        state
            .workspaces()
            .register("docs", &first.root(), WorkspaceProperties::default())
            .await
            .unwrap();
        state
            .workspaces()
            .register("notes", &second.root(), WorkspaceProperties::default())
            .await
            .unwrap();
        let app = router().with_state(state);

        let response = send(
            &app,
            json_request(
                "POST",
                "/exec",
                &json!({
                    "command": "sh",
                    "args": ["-c", "printf %s:%s \"$WORKSPACE_DOCS\" \"$WORKSPACE_NOTES\""],
                    "wait": 5000,
                }),
            ),
        )
        .await;

        assert_eq!(
            json_body(response).await["stdout"],
            json!(format!(
                "{}:{}",
                canonical(first.path()).to_str().unwrap(),
                canonical(second.path()).to_str().unwrap(),
            ))
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
    async fn schema_violations_are_invalid_requests() {
        let app = app();

        for body in [
            json!({}),
            json!({ "format": "shell" }),
            json!({ "format": 3, "command": "true" }),
        ] {
            let response = send(&app, json_request("POST", "/exec", &body)).await;

            assert_eq!(
                response.status(),
                StatusCode::UNPROCESSABLE_ENTITY,
                "unexpected status for {body}"
            );
            assert_eq!(
                json_body(response).await["error"]["code"],
                json!("invalid_request"),
                "unexpected code for {body}"
            );
        }
    }

    #[tokio::test]
    async fn an_unknown_format_is_rejected() {
        let response = send(
            &app(),
            json_request(
                "POST",
                "/exec",
                &json!({ "format": "python", "script": "print(1)" }),
            ),
        )
        .await;

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            json_body(response).await["error"]["code"],
            json!("unsupported_type")
        );
    }

    #[tokio::test]
    async fn a_zero_wait_streams_immediately() {
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
            STREAM_CONTENT_TYPE
        );

        let frames = frames(response).await;
        assert_eq!(stream_bytes(&frames, false), b"out");
        assert_eq!(stream_bytes(&frames, true), b"err");
        assert_eq!(final_status(&frames), Status::Exited { exit_code: 3 });
    }

    #[tokio::test]
    async fn a_probe_that_expires_upgrades_to_the_frame_stream() {
        let request = ExecRequest::Exec {
            command: "sh".to_owned(),
            args: vec![
                "-c".to_owned(),
                "printf out; printf err >&2; exit 3".to_owned(),
            ],
            common: ExecCommon {
                cwd: None,
                env: HashMap::new(),
                wait: 0,
            },
        };
        let stream = exec::exec(request, WorkspaceContext::default())
            .await
            .unwrap();

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
        assert_eq!(final_status(&frames), Status::Exited { exit_code: 3 });
    }

    #[tokio::test]
    async fn exec_streams_a_command_that_outlives_the_probe() {
        let response = send(
            &app(),
            json_request(
                "POST",
                "/exec",
                &json!({ "command": "sh", "args": ["-c", "sleep 1; printf late"], "wait": 100 }),
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
        assert_eq!(final_status(&frames), Status::Exited { exit_code: 0 });
    }

    #[tokio::test]
    async fn a_short_wait_upgrades_the_response() {
        let response = send(
            &app(),
            json_request(
                "POST",
                "/exec",
                &json!({
                    "command": "sh",
                    "args": ["-c", "sleep 0.3; printf late"],
                    "wait": 50,
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
    async fn a_long_wait_keeps_a_slow_command_direct() {
        let response = send(
            &app(),
            json_request(
                "POST",
                "/exec",
                &json!({
                    "command": "sh",
                    "args": ["-c", "sleep 0.3; printf late"],
                    "wait": 5000,
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
    async fn shell_honors_the_wait() {
        let response = send(
            &app(),
            json_request(
                "POST",
                "/exec",
                &json!({
                    "format": "shell",
                    "script": "sleep 0.3; printf late",
                    "wait": 50,
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
