use std::convert::Infallible;
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use bytes::BytesMut;
use tokio_stream::StreamExt;

use crate::exec::frame;
use crate::exec::model::{ExecError, ExecRequest, ExecResult};
use crate::exec::runtime::{Event, Execution};
use crate::http::{AppState, ExtractWorkspace};

pub fn routes() -> Router<AppState> {
    Router::new().route("/", post(exec))
}

async fn exec(workspace: ExtractWorkspace, Json(request): Json<ExecRequest>) -> Response {
    let metadata = workspace.metadata().await;

    let (program, args) = match request.resolve() {
        Ok(resolved) => resolved,
        Err(error) => return error.into_response(),
    };
    let cwd = request.cwd.clone().unwrap_or_else(|| metadata.root.clone());
    let env = request.environment(&metadata);

    let mut execution = match Execution::spawn(&program, &args, &cwd, &env) {
        Ok(execution) => execution,
        Err(source) => return ExecError::Spawn { program, source }.into_response(),
    };

    // A `wait` of zero never waits: the `200` carries the frame stream from its
    // first byte.
    let (consumed, result) = match request.wait {
        0 => (Vec::new(), None),
        wait => collect(&mut execution, Duration::from_millis(wait)).await,
    };

    match result {
        Some(result) => Json(result).into_response(),
        None => stream(consumed, execution),
    }
}

/// Consumes events until the terminal status or `wait` elapses. The events come
/// back either way, so an upgraded response replays what the wait consumed.
async fn collect(execution: &mut Execution, wait: Duration) -> (Vec<Event>, Option<ExecResult>) {
    let mut consumed = Vec::new();

    let deadline = tokio::time::sleep(wait);
    tokio::pin!(deadline);

    loop {
        let event = tokio::select! {
            () = &mut deadline => break,
            event = execution.recv() => event,
        };

        let Some(event) = event else {
            break;
        };

        let terminal = matches!(event, Event::Status(_));
        consumed.push(event);

        if terminal {
            break;
        }
    }

    let result = direct(&consumed);
    (consumed, result)
}

/// The direct result of the events consumed so far, `None` while the terminal
/// status is missing.
fn direct(events: &[Event]) -> Option<ExecResult> {
    let mut stdout = BytesMut::new();
    let mut stderr = BytesMut::new();
    let mut status = None;

    for event in events {
        match event {
            Event::Stdout(bytes) => stdout.extend_from_slice(bytes),
            Event::Stderr(bytes) => stderr.extend_from_slice(bytes),
            Event::Status(terminal) => status = Some(terminal.clone()),
        }
    }

    Some(ExecResult::new(status?, text(&stdout), text(&stderr)))
}

/// The upgraded answer: the events consumed while waiting, then the rest as
/// they arrive.
fn stream(consumed: Vec<Event>, execution: Execution) -> Response {
    let frames = tokio_stream::iter(consumed)
        .chain(execution.into_stream())
        .map(|event| Ok::<_, Infallible>(frame::encode(event)));

    (
        [(header::CONTENT_TYPE, frame::CONTENT_TYPE)],
        Body::from_stream(frames),
    )
        .into_response()
}

/// Output is captured as text, so bytes that are not UTF-8 are replaced.
fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// The payload is the caller's to fix, whether it was incomplete or named a
/// program that could not be started.
impl IntoResponse for ExecError {
    fn into_response(self) -> Response {
        (StatusCode::BAD_REQUEST, self.to_string()).into_response()
    }
}
