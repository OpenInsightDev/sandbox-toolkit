use hyper_util::rt::TokioIo;
use salvo::extract::JsonBody;
use salvo::http::uri::Uri;
use salvo::http::{Method, StatusCode};
use salvo::prelude::*;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::protocol::Role;

use crate::http::{ExtractWorkspace, app_state};
use crate::pty::{PtyError, PtyRequest, PtySession, Session};

pub fn routes() -> Router {
    Router::new()
        .post(create)
        // Attaching is an HTTP/2 extended CONNECT (RFC 8441), which only that
        // method reaches; the handler runs its own upgrade.
        .push(Router::with_path("{session_id}").goal(attach))
}

#[handler]
async fn create(
    workspace: ExtractWorkspace,
    body: JsonBody<PtyRequest>,
    req: &mut Request,
    depot: &mut Depot,
) -> Result<(StatusCode, Json<PtySession>), StatusError> {
    let state = app_state(depot)?;
    let metadata = workspace.metadata().await;

    let process = body.0.resolve(&metadata, &state.bin).map_err(StatusError::from)?;
    let session = Session::spawn(&metadata.id, &process)
        .await
        .map_err(|source| StatusError::from(PtyError::Spawn { program: process.program, source }))?;

    let id = state.pty.create(session).await;
    // A session is attached under the mount that created it, which this request
    // names itself.
    let endpoint = format!("{}/{id}", mount(req.uri()));

    Ok((StatusCode::CREATED, Json(PtySession { id, endpoint })))
}

#[handler]
async fn attach(
    workspace: ExtractWorkspace,
    req: &mut Request,
    depot: &mut Depot,
    res: &mut Response,
) -> Result<(), StatusError> {
    let state = app_state(depot)?;
    let protocol = req.extensions().get::<hyper::ext::Protocol>();
    if req.method() != Method::CONNECT || protocol.map(|p| p.as_str()) != Some("websocket") {
        return Err(StatusError::not_found());
    }

    let session_id = req
        .param::<String>("session_id")
        .ok_or_else(StatusError::not_found)?;
    let scope = workspace.metadata().await.id;
    let Some((session, attachment)) = state.pty.attach(&scope, &session_id).await else {
        return Err(StatusError::not_found());
    };

    let on_upgrade = req
        .extensions_mut()
        .remove::<hyper::upgrade::OnUpgrade>()
        .ok_or_else(|| StatusError::internal_server_error().cause("connection is not upgradable"))?;

    // RFC 8441 marks a successful extended CONNECT with a 2xx status, not 101.
    res.status_code(StatusCode::OK);
    tokio::spawn(async move {
        match on_upgrade.await {
            Ok(upgraded) => {
                let socket = WebSocketStream::from_raw_socket(
                    TokioIo::new(upgraded),
                    Role::Server,
                    None,
                )
                .await;
                session.run(socket, attachment).await;
            }
            // An upgrade that never completes leaves a session no client can
            // end, so it is reclaimed instead of waiting out its idle deadline.
            Err(_) => session.reclaim(),
        }
    });

    Ok(())
}

fn mount(uri: &Uri) -> &str {
    uri.path().trim_end_matches('/')
}

/// The payload is the caller's to fix, whether it was incomplete or named a
/// program that could not be started.
impl From<PtyError> for StatusError {
    fn from(error: PtyError) -> Self {
        StatusError::bad_request().brief(error.to_string())
    }
}
