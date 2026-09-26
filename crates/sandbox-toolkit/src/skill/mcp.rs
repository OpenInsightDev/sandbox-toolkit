use rmcp::ErrorData;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::{tool, tool_router};
use serde_json::json;

use super::model::{GetRequest, ListRequest, SkillBody, SkillList};
use super::skill::{self, SkillError};
use crate::server::ToolkitServer;

#[tool_router(router = skill_tools, vis = "pub(crate)")]
impl ToolkitServer {
    /// List the skills discovered at a mount.
    #[tool(name = "list_skills")]
    async fn list_skills(
        &self,
        Parameters(request): Parameters<ListRequest>,
    ) -> Result<Json<SkillList>, ErrorData> {
        let skills = skill::list(self.state(), request)
            .await
            .map_err(skill_error)?;

        Ok(Json(skills))
    }

    /// Read one skill's `SKILL.md` body.
    #[tool(name = "get_skill")]
    async fn get_skill(
        &self,
        Parameters(request): Parameters<GetRequest>,
    ) -> Result<Json<SkillBody>, ErrorData> {
        let document = skill::body(self.state(), request)
            .await
            .map_err(skill_error)?;

        Ok(Json(document))
    }
}

/// Mirrors the HTTP status class; `data.code` carries the same stable code.
fn skill_error(error: SkillError) -> ErrorData {
    let not_found = matches!(
        error,
        SkillError::WorkspaceNotFound(_) | SkillError::SkillNotFound(_)
    );
    let message = error.to_string();
    let data = Some(json!({ "code": if not_found { "not_found" } else { "bad_request" } }));

    if not_found {
        ErrorData::resource_not_found(message, data)
    } else {
        ErrorData::invalid_params(message, data)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use rmcp::handler::server::wrapper::Parameters;

    use super::*;
    use crate::AppState;
    use crate::workspace::registry::test_support::TempDir;

    fn write_skill(skills_dir: &Path, id: &str, frontmatter: &str, body: &str) {
        let dir = skills_dir.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\n{frontmatter}\n---\n\n{body}"),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn tools_mirror_the_query_surface() {
        let agents = TempDir::new();
        write_skill(
            &agents.path().join("skills"),
            "deploy",
            "name: deploy\ndescription: Roll out a service.",
            "Ship it.",
        );
        let server = ToolkitServer::new(AppState::with_agents_dir(".", agents.path()));

        let list = server
            .list_skills(Parameters(ListRequest::default()))
            .await
            .unwrap();
        assert_eq!(list.0.skills.len(), 1);
        assert_eq!(list.0.skills[0].id, "deploy");

        let body = server
            .get_skill(Parameters(GetRequest {
                workspace_id: None,
                skill_id: "deploy".to_owned(),
            }))
            .await
            .unwrap();
        assert_eq!(body.0.content, "Ship it.");
    }

    #[tokio::test]
    async fn errors_carry_the_stable_code() {
        let agents = TempDir::new();
        let server = ToolkitServer::new(AppState::with_agents_dir(".", agents.path()));

        let missing = match server
            .get_skill(Parameters(GetRequest {
                workspace_id: None,
                skill_id: "missing".to_owned(),
            }))
            .await
        {
            Err(error) => error,
            Ok(_) => panic!("a missing skill must fail"),
        };
        assert_eq!(missing.data.unwrap()["code"], "not_found");

        let malformed = match server
            .get_skill(Parameters(GetRequest {
                workspace_id: None,
                skill_id: "Bad".to_owned(),
            }))
            .await
        {
            Err(error) => error,
            Ok(_) => panic!("a malformed skill id must fail"),
        };
        assert_eq!(malformed.data.unwrap()["code"], "bad_request");
    }
}
