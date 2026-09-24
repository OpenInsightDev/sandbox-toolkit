//! Server bootstrap and TCP listener.

use anyhow::{Context, Result};
use axum::Router;
use axum::http::Uri;
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;
use tracing::info;

use crate::{AppError, AppState, Cli, fs, mcp, process, workspace};

/// Build the API router.
fn api_router() -> Router<AppState> {
    Router::new()
        .merge(workspace::router())
        .merge(fs::router())
        .merge(process::router())
        .merge(mcp::router())
        // Every error response carries the JSON envelope described in
        // `docs/design/FileSystem.md`, including requests that match no route.
        .fallback(not_found)
}

/// Fallback for requests that match no route.
async fn not_found(uri: Uri) -> AppError {
    AppError::NotFound(uri.path().to_owned())
}

/// Run the HTTP server until it is shut down.
pub(crate) async fn run(cli: Cli) -> Result<()> {
    let state = AppState::new(cli.root.clone());
    let addr = cli.socket_addr();
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind HTTP listener to {addr}"))?;

    info!(%addr, root = %state.root().display(), "sandbox-toolkit server listening");

    let app = api_router()
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    // Without graceful shutdown, the returned future's output is uninhabited:
    // it never resolves and surfaces no error.
    match axum::serve(listener, app).await {}
}
