use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::extract::OriginalUri;
use axum::extract::Path;
use axum::extract::State;
use axum::extract::ws::WebSocketUpgrade;
use axum::http::StatusCode;
use axum::http::Uri;
use axum::response::IntoResponse;
use axum::response::Response;
use axum::routing::MethodFilter;
use axum::routing::on;
use axum::routing::post;
use serde::Deserialize;

use crate::http::AppState;
use crate::http::ExtractWorkspace;
use crate::pty::PtyError;
use crate::pty::PtyRequest;
use crate::pty::PtySession;
use crate::pty::Session;

/// Named rather than positional: a sibling capture such as `{workspace_id}` on
/// the enclosing mount lands in the same set.
#[derive(Debug, Deserialize)]
struct PtyPath {
    session_id: String,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", post(create))
        // Attaching is an HTTP/2 extended CONNECT (RFC 8441), so only that
        // method routes here; the upgrade extractor runs its handshake.
        .route("/{session_id}", on(MethodFilter::CONNECT, attach))
}

async fn create(
    State(state): State<AppState>,
    workspace: ExtractWorkspace,
    OriginalUri(uri): OriginalUri,
    Json(request): Json<PtyRequest>,
) -> Response {
    let metadata = workspace.metadata().await;

    let process = match request.resolve(&metadata, &state.bin) {
        Ok(process) => process,
        Err(error) => return error.into_response(),
    };

    let session = match Session::spawn(&metadata.id, &process).await {
        Ok(session) => session,
        Err(source) => {
            return PtyError::Spawn {
                program: process.program,
                source,
            }
            .into_response();
        }
    };

    let id = state.pty.create(session).await;
    // A session is attached under the mount that created it, which this request
    // names itself.
    let endpoint = format!("{}/{id}", mount(&uri));

    (StatusCode::CREATED, Json(PtySession { id, endpoint })).into_response()
}

async fn attach(
    State(state): State<AppState>,
    workspace: ExtractWorkspace,
    Path(path): Path<PtyPath>,
    ws: WebSocketUpgrade,
) -> Response {
    let scope = workspace.metadata().await.id;
    let Some((session, attachment)) = state.pty.attach(&scope, &path.session_id).await else {
        return StatusCode::NOT_FOUND.into_response();
    };

    // An upgrade that never completes leaves a session no client can end, so it
    // is reclaimed through the failure hook instead of waiting out its idle
    // deadline. The attachment goes with it.
    let unattached = Arc::clone(&session);
    ws.on_failed_upgrade(move |_| unattached.reclaim())
        .on_upgrade(move |socket| async move { session.run(socket, attachment).await })
}

/// The mount the create request reached, without any trailing separator.
fn mount(uri: &Uri) -> &str {
    uri.path().trim_end_matches('/')
}

/// The payload is the caller's to fix, whether it was incomplete or named a
/// program that could not be started.
impl IntoResponse for PtyError {
    fn into_response(self) -> Response {
        (StatusCode::BAD_REQUEST, self.to_string()).into_response()
    }
}
