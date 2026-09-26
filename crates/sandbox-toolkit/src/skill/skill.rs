use std::collections::HashMap;
use std::path::{Path, PathBuf};

use agent_plugins::{SkillMeta, parse_skill_md};
use thiserror::Error;

use super::model::{GetRequest, ListRequest, SkillBody, SkillList, SkillMetadata};
use crate::AppState;
use crate::workspace::registry::{Workspace, WorkspaceError, validate_id};

#[derive(Debug, Error)]
pub(crate) enum SkillError {
    #[error("invalid skill id `{0}`")]
    InvalidSkillId(String),
    #[error("invalid workspace id `{0}`")]
    InvalidWorkspaceId(String),
    #[error("workspace `{0}` does not exist")]
    WorkspaceNotFound(String),
    #[error("skill `{0}` does not exist")]
    SkillNotFound(String),
    #[error("bad request: {0}")]
    BadRequest(String),
}

struct Skill {
    /// Equals `meta.name`; the parser enforces it.
    id: String,
    /// Canonicalized so a derived workspace root cannot escape the mount.
    root: PathBuf,
    meta: SkillMeta,
    body: String,
}

struct Mount {
    workspace_id: Option<String>,
    skills: Vec<Skill>,
}

pub(crate) async fn list(state: &AppState, request: ListRequest) -> Result<SkillList, SkillError> {
    if request.limit == Some(0) {
        return Err(SkillError::BadRequest(
            "`limit` must be positive".to_owned(),
        ));
    }

    let mount = mount_for(state, request.workspace_id.as_deref()).await?;
    let total = mount.skills.len() as u64;
    let offset = request.offset.unwrap_or(0).min(total) as usize;
    let skills = mount.skills.into_iter().skip(offset);
    let skills: Vec<Skill> = match request.limit {
        Some(limit) => skills.take(limit as usize).collect(),
        None => skills.collect(),
    };

    let metadata = skills
        .into_iter()
        .map(|skill| metadata(&mount.workspace_id, skill))
        .collect();

    Ok(SkillList { skills: metadata })
}

pub(crate) async fn body(state: &AppState, request: GetRequest) -> Result<SkillBody, SkillError> {
    validate_skill_id(&request.skill_id)?;

    let mount = mount_for(state, request.workspace_id.as_deref()).await?;
    let skill = mount
        .skills
        .into_iter()
        .find(|skill| skill.id == request.skill_id)
        .ok_or_else(|| SkillError::SkillNotFound(request.skill_id.clone()))?;

    Ok(SkillBody {
        skill_id: skill.id,
        content: skill.body,
    })
}

async fn mount_for(state: &AppState, workspace_id: Option<&str>) -> Result<Mount, SkillError> {
    match workspace_id {
        Some(id) => {
            validate_id(id).map_err(|_| SkillError::InvalidWorkspaceId(id.to_owned()))?;
            let workspace = state
                .resolve_workspace(id)
                .await
                .map_err(|error| match error {
                    WorkspaceError::NotFound { .. } => SkillError::WorkspaceNotFound(id.to_owned()),
                    _ => SkillError::InvalidWorkspaceId(id.to_owned()),
                })?;
            let skills = discover(&workspace.root().join(".agents/skills")).await;

            Ok(Mount {
                workspace_id: Some(id.to_owned()),
                skills,
            })
        }
        None => {
            let skills = discover(&state.agents_dir().join("skills")).await;

            Ok(Mount {
                workspace_id: None,
                skills,
            })
        }
    }
}

/// Applies the name rules before lookup, so a malformed id is a `400` rather
/// than an unknown `404`.
fn validate_skill_id(id: &str) -> Result<(), SkillError> {
    if id.chars().count() > 64 || validate_id(id).is_err() {
        return Err(SkillError::InvalidSkillId(id.to_owned()));
    }

    Ok(())
}

fn metadata(workspace_id: &Option<String>, skill: Skill) -> SkillMetadata {
    let uri = match workspace_id {
        Some(workspace_id) => format!("/workspaces/{workspace_id}/skills/{}", skill.id),
        None => format!("/skills/{}", skill.id),
    };
    let workspace_id = derived_workspace_id(workspace_id.as_deref(), &skill.id);
    let metadata = skill.meta.metadata.into_iter().collect::<HashMap<_, _>>();

    SkillMetadata {
        id: skill.id,
        root: skill.root.display().to_string(),
        name: skill.meta.name,
        description: skill.meta.description,
        license: skill.meta.license,
        compatibility: skill.meta.compatibility,
        metadata: (!metadata.is_empty()).then_some(metadata),
        uri,
        workspace_id,
    }
}

/// A missing or unreadable directory is an empty mount, not an error.
async fn discover(skills_dir: &Path) -> Vec<Skill> {
    let mut skills = Vec::new();
    let mut entries = match tokio::fs::read_dir(skills_dir).await {
        Ok(entries) => entries,
        Err(_) => return skills,
    };

    while let Ok(Some(entry)) = entries.next_entry().await {
        // A symlinked child is not followed, so a skill cannot point outside
        // the mount.
        let Ok(file_type) = entry.file_type().await else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }

        let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if let Some(skill) = read_skill(&entry.path(), id).await {
            skills.push(skill);
        }
    }

    skills.sort_by(|a, b| a.id.cmp(&b.id));

    skills
}

async fn read_skill(dir: &Path, id: String) -> Option<Skill> {
    let bytes = tokio::fs::read(dir.join("SKILL.md")).await.ok()?;
    let (meta, body) = parse_skill_md(&id, &bytes).ok()?;
    let root = tokio::fs::canonicalize(dir).await.ok()?;

    Some(Skill {
        id,
        root,
        meta,
        body: body.source().to_owned(),
    })
}

/// Encodes the mount into the id, so a derived workspace can be resolved from
/// the id alone instead of being registered.
pub(crate) fn derived_workspace_id(workspace_id: Option<&str>, skill_id: &str) -> String {
    match workspace_id {
        Some(workspace_id) => format!("skill-{workspace_id}-{skill_id}"),
        None => format!("skill-{skill_id}"),
    }
}

/// Scans discovery on demand, so the workspace set tracks the `.agents/skills`
/// directories.
///
/// The global mount is tried first, then the longest registered workspace prefix
/// of the remainder.
pub(crate) async fn resolve_workspace(state: &AppState, id: &str) -> Option<Workspace> {
    let remainder = id.strip_prefix("skill-")?;
    if remainder.is_empty() {
        return None;
    }

    let global = state.agents_dir().join("skills");
    if let Some(root) = skill_root(&global, remainder).await {
        return Some(Workspace::managed(id, root));
    }

    let mut workspace_ids: Vec<String> = state
        .workspaces()
        .list()
        .into_iter()
        .map(|handle| handle.id().to_owned())
        .collect();
    workspace_ids.sort_by_key(|workspace_id| std::cmp::Reverse(workspace_id.len()));

    for workspace_id in workspace_ids {
        let Some(skill_id) = remainder.strip_prefix(&format!("{workspace_id}-")) else {
            continue;
        };
        if skill_id.is_empty() {
            continue;
        }

        let Ok(workspace) = state.workspaces().workspace(&workspace_id) else {
            continue;
        };
        let skills_dir = workspace.root().join(".agents/skills");
        if let Some(root) = skill_root(&skills_dir, skill_id).await {
            return Some(Workspace::managed(id, root));
        }
    }

    None
}

async fn skill_root(skills_dir: &Path, skill_id: &str) -> Option<PathBuf> {
    discover(skills_dir)
        .await
        .into_iter()
        .find(|skill| skill.id == skill_id)
        .map(|skill| skill.root)
}

#[cfg(test)]
mod tests {
    use super::derived_workspace_id;

    #[test]
    fn derives_the_workspace_id_from_the_mount() {
        assert_eq!(derived_workspace_id(None, "deploy"), "skill-deploy");
        assert_eq!(
            derived_workspace_id(Some("docs"), "deploy"),
            "skill-docs-deploy"
        );
    }
}
