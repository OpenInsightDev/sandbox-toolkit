use std::collections::BTreeMap;
use std::sync::Arc;

use salvo::http::{HeaderValue, header};
use salvo::prelude::*;
use tokio::sync::broadcast;

use crate::events::{self, Event};
use crate::http::{origin, respond_events, wants_events};

use super::discover::{Skill, Skills};
use super::model::{SkillList, SkillMetadata};

pub struct ExtractSkills {
    global: Option<Skills>,
    scoped: Option<Skills>,
    events: Option<Arc<events::Observer>>,
}

impl ExtractSkills {
    pub fn new(
        global: Option<Skills>,
        scoped: Option<Skills>,
        events: Option<Arc<events::Observer>>,
    ) -> Self {
        Self {
            global,
            scoped,
            events,
        }
    }

    /// The events the mount's scope reports; a scope that holds no resources
    /// reports none.
    pub fn stream(&self) -> broadcast::Receiver<Event> {
        events::subscribe(self.events.as_deref())
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
                    skill.id.clone(),
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

pub fn routes() -> Router {
    Router::new()
        .get(list)
        .push(Router::with_path("{skill_id}").get(body))
}

#[handler]
async fn list(
    skills: ExtractSkills,
    req: &mut Request,
    res: &mut Response,
) -> Result<(), StatusError> {
    if wants_events(req) {
        respond_events(res, skills.stream(), events::resource_frame);

        return Ok(());
    }

    let base = format!("{}{}", origin(req.uri(), req.headers()), req.uri().path());
    let document = skills
        .merged()
        .await
        .into_iter()
        .map(|scoped| SkillMetadata::new(&scoped.scope, &scoped.skill, &base))
        .collect();
    res.render(Json(SkillList { skills: document }));

    Ok(())
}

#[handler]
async fn body(req: &mut Request, skills: ExtractSkills, res: &mut Response) -> Result<(), StatusError> {
    let id = req
        .param::<String>("skill_id")
        .ok_or_else(StatusError::not_found)?;

    match skills.get(&id).await {
        Some(skill) => {
            // The body is the `SKILL.md` text as written, so it serves as Markdown.
            res.headers_mut()
                .insert(header::CONTENT_TYPE, HeaderValue::from_static(MARKDOWN));
            res.body(skill.body);

            Ok(())
        }
        None => Err(StatusError::not_found()),
    }
}

const MARKDOWN: &str = "text/markdown";
