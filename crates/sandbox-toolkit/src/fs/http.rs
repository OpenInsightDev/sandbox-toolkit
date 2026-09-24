//! The file API endpoints.

use axum::Router;

use super::files;
use crate::AppState;

/// Build the file router.
///
/// Paths include the owning workspace because a file is always addressed
/// within one, but the routes stand on their own rather than being nested into
/// the workspace control plane.
pub(crate) fn router() -> Router<AppState> {
    // A wildcard capture never matches an empty path, and `/fs` and `/fs/`
    // are distinct routes in axum, so addressing the workspace root needs these
    // two routes; the wildcard route matches neither of them.
    Router::new()
        .route("/workspaces/{workspace_id}/fs", files::file_routes())
        .route("/workspaces/{workspace_id}/fs/", files::file_routes())
        .route(
            "/workspaces/{workspace_id}/fs/{*path}",
            files::file_routes(),
        )
        .route("/fs", files::file_routes())
        .route("/fs/", files::file_routes())
        .route("/fs/{*path}", files::file_routes())
}
