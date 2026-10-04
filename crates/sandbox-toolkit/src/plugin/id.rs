const DERIVED_PREFIX: &str = "plugin.";

/// The workspace id a plugin's extension namespace derives.
///
/// A plugin name and a namespace both allow `.`, so the parts are composed only
/// here: resolution looks the composed id up instead of splitting it apart.
pub fn extension_workspace_id(scope: &str, plugin_id: &str, namespace: &str) -> String {
    format!("{DERIVED_PREFIX}{scope}.{plugin_id}.{namespace}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_parts_compose_the_id() {
        assert_eq!(
            extension_workspace_id("global", "deploy-kit", "com.example.client"),
            "plugin.global.deploy-kit.com.example.client"
        );
    }
}
