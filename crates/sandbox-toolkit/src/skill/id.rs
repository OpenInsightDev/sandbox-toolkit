const DERIVED_PREFIX: &str = "skill.";

/// The workspace id a scope's skill derives.
///
/// A skill id carries no `.`, so the scope keeps the separators its own id may
/// hold and the last segment stays the skill.
pub fn derived_workspace_id(scope: &str, skill_id: &str) -> String {
    format!("{DERIVED_PREFIX}{scope}.{skill_id}")
}

/// The scope and skill id a derived workspace id names, `None` for any other shape.
pub fn split_derived_workspace_id(id: &str) -> Option<(&str, &str)> {
    id.strip_prefix(DERIVED_PREFIX)?.rsplit_once('.')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_derived_id_splits_into_the_scope_and_the_skill() {
        assert_eq!(
            split_derived_workspace_id("skill.docs.deploy"),
            Some(("docs", "deploy"))
        );
        assert_eq!(
            split_derived_workspace_id("skill.global.deploy"),
            Some(("global", "deploy"))
        );
        assert_eq!(
            split_derived_workspace_id("skill.plugin.docs.deploy"),
            Some(("plugin.docs", "deploy"))
        );
    }

    #[test]
    fn a_derived_id_survives_the_round_trip() {
        let id = derived_workspace_id("docs", "deploy");
        assert_eq!(split_derived_workspace_id(&id), Some(("docs", "deploy")));
    }

    #[test]
    fn other_ids_are_not_derived() {
        assert_eq!(split_derived_workspace_id("docs"), None);
        assert_eq!(split_derived_workspace_id("skill.deploy"), None);
        assert_eq!(split_derived_workspace_id("skills.docs.deploy"), None);
    }
}
