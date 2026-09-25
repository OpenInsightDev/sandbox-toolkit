//! A skill is discovered from a fixed `.agents/skills` directory rather than
//! registered, so [`http`] is the only call surface.

mod http;
pub(crate) mod model;

pub(crate) use self::http::router;

/// The id of the read-only workspace a discovered skill derives. The `skill-`
/// prefix marks a derived workspace; a skill inside a workspace carries that
/// workspace in the id, a global skill only its own.
#[allow(dead_code, reason = "used by the list handler, which is a stub")]
pub(crate) fn derived_workspace_id(workspace_id: Option<&str>, skill_id: &str) -> String {
    match workspace_id {
        Some(workspace_id) => format!("skill-{workspace_id}-{skill_id}"),
        None => format!("skill-{skill_id}"),
    }
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
