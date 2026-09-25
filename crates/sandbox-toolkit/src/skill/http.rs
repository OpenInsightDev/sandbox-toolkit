//! Every route is mounted twice: `/skills` over the global `.agents/skills`
//! directory and `/workspaces/{workspace_id}/skills` over the one inside the
//! workspace root. The collection route answers metadata, the item route the
//! `SKILL.md` body; other files are reached through the read-only workspace a
//! skill derives.

use std::collections::HashMap;

use axum::Json;
use axum::Router;
use axum::extract::{FromRequestParts, Path, Query, State};
use axum::http::Method;
use axum::http::request::Parts;
use axum::response::Response;
use axum::routing::{self, MethodRouter};

use super::model::SkillList;
use crate::{AppError, AppState};

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/skills", collection_routes())
        .route("/skills/{skill_id}", item_routes())
        .route("/workspaces/{workspace_id}/skills", collection_routes())
        .route(
            "/workspaces/{workspace_id}/skills/{skill_id}",
            item_routes(),
        )
}

fn collection_routes() -> MethodRouter<AppState> {
    routing::get(list_skills).fallback(method_not_allowed)
}

fn item_routes() -> MethodRouter<AppState> {
    routing::get(get_skill_body).fallback(method_not_allowed)
}

/// `workspace_id` is absent on the global mount; `skill_id` is absent on the
/// collection routes.
#[derive(Debug)]
struct SkillTarget {
    #[expect(dead_code, reason = "read by the skill handlers, which are stubs")]
    workspace_id: Option<String>,
    #[expect(dead_code, reason = "read by the skill handlers, which are stubs")]
    skill_id: Option<String>,
}

impl FromRequestParts<AppState> for SkillTarget {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let Path(captures): Path<HashMap<String, String>> = Path::from_request_parts(parts, state)
            .await
            .map_err(|_| AppError::BadRequest("invalid skill path".to_owned()))?;

        Ok(Self {
            workspace_id: captures.get("workspace_id").cloned(),
            skill_id: captures.get("skill_id").cloned(),
        })
    }
}

#[derive(Debug, Default, serde::Deserialize)]
struct SkillListQuery {
    #[expect(dead_code, reason = "read by the list handler, which is a stub")]
    offset: Option<u64>,
    #[expect(dead_code, reason = "read by the list handler, which is a stub")]
    limit: Option<u64>,
}

async fn list_skills(
    State(_state): State<AppState>,
    _target: SkillTarget,
    Query(_query): Query<SkillListQuery>,
) -> Result<Json<SkillList>, AppError> {
    Err(AppError::NotImplemented("GET skills"))
}

async fn get_skill_body(
    State(_state): State<AppState>,
    _target: SkillTarget,
) -> Result<Response, AppError> {
    Err(AppError::NotImplemented("GET skills/{skill_id}"))
}

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

    #[tokio::test]
    async fn read_routes_are_wired() {
        let app = app();

        for uri in [
            "/skills",
            "/skills/deploy",
            "/workspaces/docs/skills",
            "/workspaces/docs/skills/deploy",
        ] {
            let request = Request::builder()
                .method("GET")
                .uri(uri)
                .body(Body::empty())
                .unwrap();

            let (status, body) = send(&app, request).await;
            assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "GET {uri}");
            assert_eq!(body["error"]["code"], json!("not_implemented"), "GET {uri}");
        }
    }

    #[tokio::test]
    async fn rejects_unsupported_methods() {
        for uri in ["/skills", "/skills/deploy"] {
            let request = Request::builder()
                .method("DELETE")
                .uri(uri)
                .body(Body::empty())
                .unwrap();

            let (status, body) = send(&app(), request).await;
            assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED, "DELETE {uri}");
            assert_eq!(
                body["error"]["code"],
                json!("method_not_allowed"),
                "DELETE {uri}"
            );
        }
    }
}
