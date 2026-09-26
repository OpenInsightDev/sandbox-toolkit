use std::collections::HashMap;

use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::{FromRequestParts, Path, Query, State};
use axum::http::header;
use axum::http::request::Parts;
use axum::http::{HeaderValue, Method};
use axum::response::Response;
use axum::routing::{self, MethodRouter};

use super::model::{GetRequest, ListRequest, SkillList};
use super::query::{self, SkillError};
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

#[derive(Debug)]
struct SkillTarget {
    workspace_id: Option<String>,
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
    offset: Option<u64>,
    limit: Option<u64>,
}

async fn list_skills(
    State(state): State<AppState>,
    target: SkillTarget,
    Query(query): Query<SkillListQuery>,
) -> Result<Json<SkillList>, AppError> {
    let request = ListRequest {
        workspace_id: target.workspace_id,
        offset: query.offset,
        limit: query.limit,
    };

    Ok(Json(query::list(&state, request).await?))
}

async fn get_skill_body(
    State(state): State<AppState>,
    target: SkillTarget,
) -> Result<Response, AppError> {
    let skill_id = target
        .skill_id
        .ok_or_else(|| AppError::BadRequest("missing skill id".to_owned()))?;
    let request = GetRequest {
        workspace_id: target.workspace_id,
        skill_id,
    };
    let document = query::body(&state, request).await?;

    let mut response = Response::new(Body::from(document.content));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/markdown; charset=utf-8"),
    );

    Ok(response)
}

impl From<SkillError> for AppError {
    fn from(error: SkillError) -> Self {
        let message = error.to_string();

        match error {
            SkillError::InvalidSkillId(_)
            | SkillError::InvalidWorkspaceId(_)
            | SkillError::BadRequest(_) => Self::BadRequest(message),
            SkillError::WorkspaceNotFound(_) | SkillError::SkillNotFound(_) => {
                Self::NotFound(message)
            }
        }
    }
}

async fn method_not_allowed(method: Method) -> AppError {
    AppError::MethodNotAllowed(method)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use axum::http::{HeaderMap, Request, StatusCode};
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::*;
    use crate::workspace::model::WorkspaceProperties;
    use crate::workspace::registry::test_support::TempDir;

    /// The global and workspace mounts plus the filesystem and workspace routes,
    /// which is how a derived workspace is actually reached.
    fn app(state: AppState) -> Router {
        Router::new()
            .merge(crate::fs::router())
            .merge(crate::workspace::router())
            .merge(router())
            .with_state(state)
    }

    fn state_with_agents(agents: &TempDir) -> AppState {
        AppState::with_agents_dir(".", agents.path())
    }

    fn write_skill(skills_dir: &Path, id: &str, frontmatter: &str, body: &str) {
        let dir = skills_dir.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\n{frontmatter}\n---\n\n{body}"),
        )
        .unwrap();
    }

    async fn send(app: &Router, request: Request<Body>) -> (StatusCode, HeaderMap, Vec<u8>) {
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();

        (status, headers, bytes.to_vec())
    }

    async fn send_json(app: &Router, request: Request<Body>) -> (StatusCode, Value) {
        let (status, _, bytes) = send(app, request).await;
        let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);

        (status, body)
    }

    fn get(uri: &str) -> Request<Body> {
        Request::builder()
            .method("GET")
            .uri(uri)
            .body(Body::empty())
            .unwrap()
    }

    fn query(base: &str, kind: &str, body: Value) -> Request<Body> {
        Request::builder()
            .method("QUERY")
            .uri(format!("{base}?type={kind}"))
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    #[tokio::test]
    async fn lists_the_global_skills_and_skips_invalid_children() {
        let agents = TempDir::new();
        let skills = agents.path().join("skills");
        write_skill(
            &skills,
            "deploy",
            "name: deploy\ndescription: Roll out a service.\nlicense: MIT\ncompatibility: Requires kubectl\nmetadata:\n  author: acme",
            "Ship it.",
        );
        write_skill(
            &skills,
            "other",
            "name: other\ndescription: Another skill.",
            "Other body.",
        );
        // A directory without SKILL.md is skipped.
        std::fs::create_dir_all(skills.join("empty")).unwrap();
        // A frontmatter name that disagrees with the directory is skipped.
        write_skill(
            &skills,
            "mismatch",
            "name: different\ndescription: No.",
            "No.",
        );
        // A non-directory child is skipped.
        std::fs::write(skills.join("loose.txt"), "not a skill").unwrap();

        let app = app(state_with_agents(&agents));
        let (status, body) = send_json(&app, get("/skills")).await;

        assert_eq!(status, StatusCode::OK);
        let listed = body["skills"].as_array().unwrap();
        assert_eq!(listed.len(), 2, "{body}");
        assert_eq!(listed[0]["id"], "deploy");
        assert_eq!(listed[1]["id"], "other");

        let deploy = &listed[0];
        assert_eq!(deploy["name"], "deploy");
        assert_eq!(deploy["description"], "Roll out a service.");
        assert_eq!(deploy["license"], "MIT");
        assert_eq!(deploy["compatibility"], "Requires kubectl");
        assert_eq!(deploy["metadata"], json!({ "author": "acme" }));
        assert_eq!(deploy["uri"], "/skills/deploy");
        assert_eq!(deploy["workspace_id"], "skill-deploy");
        assert_eq!(
            deploy["root"],
            std::fs::canonicalize(skills.join("deploy"))
                .unwrap()
                .display()
                .to_string()
        );

        // A skill without optional fields omits them.
        assert!(listed[1].get("license").is_none());
        assert!(listed[1].get("metadata").is_none());
    }

    #[tokio::test]
    async fn gets_the_body_without_the_frontmatter() {
        let agents = TempDir::new();
        let skills = agents.path().join("skills");
        write_skill(
            &skills,
            "deploy",
            "name: deploy\ndescription: Roll out a service.",
            "# Deploy\n\nShip it.\n",
        );

        let app = app(state_with_agents(&agents));
        let (status, headers, bytes) = send(&app, get("/skills/deploy")).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            headers[header::CONTENT_TYPE],
            "text/markdown; charset=utf-8"
        );
        assert_eq!(
            std::str::from_utf8(&bytes).unwrap(),
            "# Deploy\n\nShip it.\n"
        );
    }

    #[tokio::test]
    async fn reports_unknown_and_malformed_skill_ids() {
        let agents = TempDir::new();
        write_skill(
            &agents.path().join("skills"),
            "deploy",
            "name: deploy\ndescription: Roll out a service.",
            "Ship it.",
        );

        let app = app(state_with_agents(&agents));

        let (status, body) = send_json(&app, get("/skills/missing")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"]["code"], json!("not_found"));

        // The charset and hyphen rules reject a malformed id before lookup.
        for uri in ["/skills/Bad", "/skills/-bad", "/skills/a--b"] {
            let (status, body) = send_json(&app, get(uri)).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "GET {uri}");
            assert_eq!(body["error"]["code"], json!("bad_request"), "GET {uri}");
        }
    }

    #[tokio::test]
    async fn lists_and_gets_skills_under_a_workspace_mount() {
        let agents = TempDir::new();
        let root = TempDir::new();
        write_skill(
            &root.path().join(".agents/skills"),
            "deploy",
            "name: deploy\ndescription: Roll out a service.",
            "Workspace body.",
        );

        let state = state_with_agents(&agents);
        state
            .workspaces()
            .register("docs", &root.root(), WorkspaceProperties::default())
            .await
            .unwrap();
        let app = app(state);

        let (status, body) = send_json(&app, get("/workspaces/docs/skills")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["skills"][0]["uri"], "/workspaces/docs/skills/deploy");
        assert_eq!(body["skills"][0]["workspace_id"], "skill-docs-deploy");

        let (status, _, bytes) = send(&app, get("/workspaces/docs/skills/deploy")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(std::str::from_utf8(&bytes).unwrap(), "Workspace body.");

        // An unknown workspace and a malformed one are distinct.
        let (status, body) = send_json(&app, get("/workspaces/missing/skills")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"]["code"], json!("not_found"));

        let (status, body) = send_json(&app, get("/workspaces/Bad/skills")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"]["code"], json!("bad_request"));
    }

    #[tokio::test]
    async fn paginates_the_listing() {
        let agents = TempDir::new();
        let skills = agents.path().join("skills");
        for id in ["alpha", "bravo", "charlie"] {
            write_skill(
                &skills,
                id,
                &format!("name: {id}\ndescription: A skill."),
                "Body.",
            );
        }

        let app = app(state_with_agents(&agents));

        let (status, body) = send_json(&app, get("/skills?offset=1&limit=1")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["skills"].as_array().unwrap().len(), 1);
        assert_eq!(body["skills"][0]["id"], "bravo");

        let (status, body) = send_json(&app, get("/skills?limit=0")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"]["code"], json!("bad_request"));
    }

    #[tokio::test]
    async fn serves_the_derived_workspace_read_only() {
        let agents = TempDir::new();
        let skills = agents.path().join("skills");
        write_skill(
            &skills,
            "deploy",
            "name: deploy\ndescription: Roll out a service.",
            "Body.",
        );
        std::fs::write(skills.join("deploy/run.sh"), "echo hi\n").unwrap();

        let app = app(state_with_agents(&agents));

        // Other files are reached through the derived workspace.
        let (status, body) = send_json(
            &app,
            query(
                "/workspaces/skill-deploy/fs",
                "content",
                json!({ "path": "run.sh" }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["content"], "echo hi\n");

        let request = Request::builder()
            .method("PUT")
            .uri("/workspaces/skill-deploy/fs?type=file")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({ "path": "x.txt", "content": "x" }).to_string(),
            ))
            .unwrap();
        let (status, body) = send_json(&app, request).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["error"]["code"], json!("read_only_workspace"));
    }

    #[tokio::test]
    async fn reports_a_derived_workspace_as_managed() {
        let agents = TempDir::new();
        write_skill(
            &agents.path().join("skills"),
            "deploy",
            "name: deploy\ndescription: Roll out a service.",
            "Body.",
        );

        let app = app(state_with_agents(&agents));

        let (status, body) = send_json(&app, get("/workspaces/skill-deploy")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["properties"]["access"], "read-only");

        let request = Request::builder()
            .method("DELETE")
            .uri("/workspaces/skill-deploy")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send_json(&app, request).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["error"]["code"], json!("managed_workspace"));

        // The skill workspace survives the rejected delete.
        let (status, _) = send_json(&app, get("/workspaces/skill-deploy")).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn rejects_unsupported_methods() {
        let agents = TempDir::new();
        let app = app(state_with_agents(&agents));

        for uri in ["/skills", "/skills/deploy"] {
            let request = Request::builder()
                .method("DELETE")
                .uri(uri)
                .body(Body::empty())
                .unwrap();

            let (status, body) = send_json(&app, request).await;
            assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED, "DELETE {uri}");
            assert_eq!(
                body["error"]["code"],
                json!("method_not_allowed"),
                "DELETE {uri}"
            );
        }
    }
}
