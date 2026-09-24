//! The MCP control plane and the Streamable HTTP endpoints.
//!
//! Every route is mounted twice: under a workspace prefix and directly, matching the
//! two addressing modes of [`crate::workspace`]. A registered entry is served by its
//! own `/mcp` subpath, which keeps `GET` and `DELETE` from colliding with the
//! management routes on the same resource.

use std::collections::HashMap;

use axum::Json;
use axum::Router;
use axum::extract::{FromRequestParts, Path, Query, State};
use axum::http::Method;
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::Response;
use axum::routing::{self, MethodRouter};

use super::model::{Mcp, RegisterMcpRequest};
use crate::{AppError, AppState};

/// Build the MCP router.
pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/mcps", routing::get(list_mcps).post(register_mcp))
        .route("/mcps/{mcp_id}", routing::get(get_mcp).delete(delete_mcp))
        .route("/mcps/{mcp_id}/mcp", entry_routes())
        .route(
            "/workspaces/{workspace_id}/mcps",
            routing::get(list_mcps).post(register_mcp),
        )
        .route(
            "/workspaces/{workspace_id}/mcps/{mcp_id}",
            routing::get(get_mcp).delete(delete_mcp),
        )
        .route(
            "/workspaces/{workspace_id}/mcps/{mcp_id}/mcp",
            entry_routes(),
        )
}

/// The methods of a registered entry's endpoint.
///
/// The transport occupies `POST` for JSON-RPC calls, `GET` for the server-to-client
/// stream and `DELETE` to end a session; the fallback keeps the JSON envelope for the
/// methods it does not support.
fn entry_routes() -> MethodRouter<AppState> {
    routing::post(mcp_endpoint)
        .get(mcp_endpoint)
        .delete(mcp_endpoint)
        .fallback(method_not_allowed)
}

/// The mount point an MCP route is addressed under and the entry it targets.
///
/// `workspace_id` is absent on the direct mount; `mcp_id` is absent on the collection
/// routes.
#[derive(Debug)]
struct McpTarget {
    #[expect(dead_code, reason = "read by the MCP handlers, which are stubs")]
    workspace_id: Option<String>,
    #[expect(dead_code, reason = "read by the MCP handlers, which are stubs")]
    mcp_id: Option<String>,
}

impl FromRequestParts<AppState> for McpTarget {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let Path(captures): Path<HashMap<String, String>> = Path::from_request_parts(parts, state)
            .await
            .map_err(|_| AppError::BadRequest("invalid MCP path".to_owned()))?;

        Ok(Self {
            workspace_id: captures.get("workspace_id").cloned(),
            mcp_id: captures.get("mcp_id").cloned(),
        })
    }
}

/// Query parameters of the collection routes.
#[derive(Debug, Default, serde::Deserialize)]
struct McpListQuery {
    /// `mcp-json` selects the export instead of the resource list.
    #[expect(dead_code, reason = "read by the list handler, which is a stub")]
    format: Option<String>,
}

async fn register_mcp(
    State(_state): State<AppState>,
    _target: McpTarget,
    Json(_request): Json<RegisterMcpRequest>,
) -> Result<(StatusCode, Json<Mcp>), AppError> {
    Err(AppError::NotImplemented("POST mcps"))
}

/// List the MCP servers under the mount point, or export them when `format=mcp-json`.
async fn list_mcps(
    State(_state): State<AppState>,
    _target: McpTarget,
    Query(_query): Query<McpListQuery>,
) -> Result<Response, AppError> {
    Err(AppError::NotImplemented("GET mcps"))
}

async fn get_mcp(
    State(_state): State<AppState>,
    _target: McpTarget,
) -> Result<Json<Mcp>, AppError> {
    Err(AppError::NotImplemented("GET mcps/{mcp_id}"))
}

async fn delete_mcp(
    State(_state): State<AppState>,
    _target: McpTarget,
) -> Result<StatusCode, AppError> {
    Err(AppError::NotImplemented("DELETE mcps/{mcp_id}"))
}

/// Serve a registered entry over Streamable HTTP, hiding the transport it declared.
async fn mcp_endpoint(_target: McpTarget) -> Result<Response, AppError> {
    Err(AppError::NotImplemented("MCP endpoint"))
}

/// Fallback for the methods an entry's endpoint does not support.
async fn method_not_allowed(method: Method) -> AppError {
    AppError::MethodNotAllowed(method)
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::*;

    /// The router with a fresh state, ready to drive through `oneshot`.
    fn app() -> Router {
        router().with_state(AppState::new("."))
    }

    async fn send(app: &Router, request: Request<Body>) -> (StatusCode, Value) {
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        };

        (status, body)
    }

    fn json_request(method: &str, uri: &str, body: &Value) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    #[tokio::test]
    async fn control_routes_are_wired() {
        let app = app();
        let register = json!({
            "id": "validator",
            "server": { "type": "stdio", "command": "./bin/validator" },
        });

        for (method, uri) in [
            ("POST", "/mcps"),
            ("GET", "/mcps"),
            ("GET", "/mcps/validator"),
            ("DELETE", "/mcps/validator"),
            ("POST", "/workspaces/docs/mcps"),
            ("GET", "/workspaces/docs/mcps"),
            ("GET", "/workspaces/docs/mcps/validator"),
            ("DELETE", "/workspaces/docs/mcps/validator"),
        ] {
            let request = if method == "POST" {
                json_request(method, uri, &register)
            } else {
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap()
            };

            let (status, body) = send(&app, request).await;
            assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{method} {uri}");
            assert_eq!(
                body["error"]["code"],
                json!("not_implemented"),
                "{method} {uri}"
            );
        }
    }

    #[tokio::test]
    async fn entry_routes_are_wired() {
        let app = app();

        for uri in ["/mcps/validator/mcp", "/workspaces/docs/mcps/validator/mcp"] {
            for method in ["POST", "GET", "DELETE"] {
                let request = Request::builder()
                    .method(method)
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap();

                let (status, body) = send(&app, request).await;
                assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{method} {uri}");
                assert_eq!(
                    body["error"]["code"],
                    json!("not_implemented"),
                    "{method} {uri}"
                );
            }
        }
    }

    #[tokio::test]
    async fn entry_rejects_unsupported_methods() {
        let request = Request::builder()
            .method("PUT")
            .uri("/mcps/validator/mcp")
            .body(Body::empty())
            .unwrap();

        let (status, body) = send(&app(), request).await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(body["error"]["code"], json!("method_not_allowed"));
    }
}
