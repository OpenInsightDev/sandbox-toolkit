use std::time::Duration;

use bytes::BytesMut;
use salvo::extract::JsonBody;
use salvo::http::body::ResBody;
use salvo::http::{HeaderValue, header};
use salvo::prelude::*;
use tokio_stream::StreamExt;

use crate::exec::frame;
use crate::exec::model::{ExecError, ExecRequest, ExecResult};
use crate::exec::runtime::{Event, Execution};
use crate::http::{ExtractWorkspace, app_state};

pub fn routes() -> Router {
    Router::new().post(exec)
}

#[handler]
async fn exec(
    workspace: ExtractWorkspace,
    body: JsonBody<ExecRequest>,
    depot: &mut Depot,
    res: &mut Response,
) -> Result<(), StatusError> {
    let state = app_state(depot)?;
    let request = body.0;
    let metadata = workspace.metadata().await;

    let (program, args) = request.resolve().map_err(StatusError::from)?;
    let cwd = request.cwd.clone().unwrap_or_else(|| metadata.root.clone());
    let env = metadata.child_env(&state.bin, &request.env);

    let mut execution = Execution::spawn(&program, &args, &cwd, &env)
        .map_err(|source| StatusError::from(ExecError::Spawn { program, source }))?;

    // A `wait` of zero never waits: the `200` carries the frame stream from its
    // first byte.
    let (consumed, result) = match request.wait {
        0 => (Vec::new(), None),
        wait => collect(&mut execution, Duration::from_millis(wait)).await,
    };

    match result {
        Some(result) => {
            res.render(Json(result));

            Ok(())
        }
        None => {
            let frames = tokio_stream::iter(consumed)
                .chain(execution.into_stream())
                .map(|event| Ok::<_, salvo::Error>(frame::encode(event)));

            res.headers_mut()
                .insert(header::CONTENT_TYPE, HeaderValue::from_static(frame::CONTENT_TYPE));
            res.body(ResBody::stream(frames));

            Ok(())
        }
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

/// Output is captured as text, so bytes that are not UTF-8 are replaced.
fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// The payload is the caller's to fix, whether it was incomplete or named a
/// program that could not be started.
impl From<ExecError> for StatusError {
    fn from(error: ExecError) -> Self {
        StatusError::bad_request().brief(error.to_string())
    }
}
