use std::collections::BTreeMap;

use axum::Json;
use axum::Router;
use axum::extract::{FromRequestParts, OriginalUri, Path};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde::Deserialize;

use crate::http::origin;

use super::discover::{Skill, Skills};
use super::model::{SkillList, SkillMetadata};

pub struct ExtractSkills {
    global: Option<Skills>,
    scoped: Option<Skills>,
}

impl ExtractSkills {
    pub fn new(global: Option<Skills>, scoped: Option<Skills>) -> Self {
        Self { global, scoped }
    }

    pub async fn get(&self, id: &str) -> Option<Skill> {
        for skills in [self.scoped.as_ref(), self.global.as_ref()]
            .into_iter()
            .flatten()
        {
            if let Some(skill) = skills.get(id).await {
                return Some(skill);
            }
        }

        None
    }

    pub async fn merged(&self) -> Vec<Scoped> {
        let mut merged: BTreeMap<String, Scoped> = BTreeMap::new();
        for skills in [self.global.as_ref(), self.scoped.as_ref()]
            .into_iter()
            .flatten()
        {
            for skill in skills.list().await {
                merged.insert(
                    skill.id().to_owned(),
                    Scoped {
                        scope: skills.scope().to_owned(),
                        skill,
                    },
                );
            }
        }

        merged.into_values().collect()
    }
}

pub struct Scoped {
    pub scope: String,
    pub skill: Skill,
}

/// Named rather than positional: sibling captures such as `{workspace_id}` on the
/// enclosing mount land in the same set.
#[derive(Debug, Deserialize)]
struct SkillPath {
    skill_id: String,
}

pub fn routes<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
    ExtractSkills: FromRequestParts<S>,
{
    Router::new()
        .route("/", get(list))
        .route("/{skill_id}", get(body))
}

async fn list(
    skills: ExtractSkills,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Response {
    let base = format!("{}{}", origin(&uri, &headers), uri.path());
    let skills = skills
        .merged()
        .await
        .into_iter()
        .map(|scoped| SkillMetadata::new(&scoped.scope, &scoped.skill, &base))
        .collect();

    Json(SkillList { skills }).into_response()
}

async fn body(skills: ExtractSkills, Path(path): Path<SkillPath>) -> Response {
    match skills.get(&path.skill_id).await {
        Some(skill) => ([(header::CONTENT_TYPE, MARKDOWN)], skill.body).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// The body is the `SKILL.md` text as written, so it serves as Markdown.
const MARKDOWN: &str = "text/markdown";
