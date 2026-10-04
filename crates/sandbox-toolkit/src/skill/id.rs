const DERIVED_PREFIX: &str = "skill.";

/// The workspace id a scope's skill derives.
pub fn derived_workspace_id(scope: &str, skill_id: &str) -> String {
    format!("{DERIVED_PREFIX}{scope}.{skill_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scope_and_skill_compose_the_id() {
        assert_eq!(derived_workspace_id("docs", "deploy"), "skill.docs.deploy");
        // A skill shipped by a plugin keeps its `{plugin_id}.` prefix whole.
        assert_eq!(
            derived_workspace_id("global", "deploy-kit.deploy"),
            "skill.global.deploy-kit.deploy"
        );
    }
}
