use axum::Router;
use axum::extract::{OriginalUri, Request, State};
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::any;

use crate::http::AppState;

/// The mount and everything below it are the same proxy, so the upload URL tusd
/// answers with is reachable where the client was told it is.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", any(proxy))
        .route("/{*rest}", any(proxy))
}

async fn proxy(
    State(state): State<AppState>,
    OriginalUri(original): OriginalUri,
    mut request: Request,
) -> Response {
    // Routing a nested mount takes the prefix off the path, which would leave
    // tusd serving a mount that is not the one the client asked for.
    *request.uri_mut() = origin_form(original);

    match state.tus.forward(request).await {
        Ok(response) => response,
        Err(error) => {
            tracing::debug!(%error, "the tus sidecar is unreachable");

            StatusCode::BAD_GATEWAY.into_response()
        }
    }
}

/// The path and query of `uri`, which is what an origin-form request line
/// carries upstream.
fn origin_form(uri: Uri) -> Uri {
    let mut parts = uri.into_parts();
    parts.scheme = None;
    parts.authority = None;

    Uri::from_parts(parts).expect("a path taken from a URI is a valid URI")
}
