//! The `plugin.json` manifest and the package-level loader.
//!
//! The manifest's failure taxonomy is exact: an **unknown top-level field**
//! and a **non-object `extensions`** are reported and ignored — the package
//! still loads; *every other* schema violation is fatal, and no component may
//! be discovered or executed. [`Manifest::parse`] returns that line as its
//! type.
//!
//! [`load`] composes the [`crate::skill`] and [`crate::mcp`] parsers over a
//! package directory and applies each resource's failure boundary: a bad
//! manifest rejects the package, a bad `mcp.json` disables that component, and
//! a bad skill or server entry skips only itself.

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::diag::{Diagnostic, Origin, Rule};
use crate::mcp;
use crate::name::{InvalidName, PluginName};
use crate::path::PackagePath;
use crate::skill;
use crate::spec::SpecVersion;

/// The canonical `$schema` identifier for Agent Plugins 1.0.0 manifests.
pub const PLUGIN_SCHEMA_1_0_0: &str = "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json";

/// A validated plugin manifest.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Manifest {
    /// The Agent Plugins version declared by `$schema`.
    pub spec: SpecVersion,
    /// The validated plugin name.
    pub name: PluginName,
    /// Version string; Semantic Versioning recommended, not enforced.
    pub version: Option<String>,
    /// Short description of plugin purpose.
    pub description: Option<String>,
    /// Author metadata.
    pub author: Option<Author>,
    /// Documentation or homepage URL. Opaque.
    pub homepage: Option<String>,
    /// Source repository URL. Opaque.
    pub repository: Option<String>,
    /// License identifier; SPDX recommended, not enforced.
    pub license: Option<String>,
    /// Search and discovery tags.
    pub keywords: Vec<String>,
    /// Client-specific manifest data, keyed by extension namespace.
    pub extensions: Extensions,
}

/// Author metadata: a closed object of three optional strings.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub struct Author {
    /// Author name.
    pub name: Option<String>,
    /// Author email address. Opaque.
    pub email: Option<String>,
    /// Author URL. Opaque.
    pub url: Option<String>,
}

/// Client extension data from the manifest `extensions` field.
///
/// Namespaces this client does not implement are carried verbatim and never
/// validated — the specification forbids a client from judging another
/// client's data. Each namespace's object is kept as its JSON text, which is
/// both the verbatim carry the specification asks for and a representation
/// that can derive `Hash` and `Eq`, unlike a `serde_json::Value`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Extensions {
    entries: BTreeMap<String, String>,
}

impl Extensions {
    /// The JSON text of a namespace's data object, if the manifest carries one.
    pub fn raw(&self, ns: &str) -> Option<&str> {
        self.entries.get(ns).map(String::as_str)
    }

    /// All declared namespaces and their JSON text, in manifest order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries.iter().map(|(ns, data)| (ns.as_str(), data.as_str()))
    }

    /// Whether no extension data was declared.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// A fatal manifest problem: the package MUST be rejected and none of its
/// components discovered or executed.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ManifestRejection {
    /// `plugin.json` is not valid JSON.
    NotJson {
        /// The JSON parser's explanation.
        detail: String,
    },
    /// The document is valid JSON but not an object.
    NotAnObject,
    /// The required `$schema` field is missing or not a string.
    MissingSchema,
    /// `$schema` declares an Agent Plugins version this client does not
    /// support. The package may be valid for a future client.
    UnsupportedSchema {
        /// The `$schema` value the manifest declared.
        declared: String,
    },
    /// The required `name` field is missing or not a string.
    MissingName,
    /// The `name` field violates the name constraints.
    InvalidName(InvalidName),
    /// A permitted field has the wrong shape — the fatal remainder of the
    /// manifest schema.
    SchemaViolation {
        /// The offending field, dotted for nested fields.
        field: String,
        /// What the specification requires there.
        expected: &'static str,
    },
}

impl fmt::Display for ManifestRejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotJson { detail } => write!(f, "plugin.json is not valid JSON: {detail}"),
            Self::NotAnObject => f.write_str("plugin.json must contain a top-level JSON object"),
            Self::MissingSchema => {
                f.write_str("required field `$schema` is missing or not a string (§5.3)")
            }
            Self::UnsupportedSchema { declared } => write!(
                f,
                "`$schema` declares an unsupported Agent Plugins version: {declared} (§5.2)"
            ),
            Self::MissingName => {
                f.write_str("required field `name` is missing or not a string (§5.3)")
            }
            Self::InvalidName(err) => write!(f, "invalid plugin name (§5.5): {err}"),
            Self::SchemaViolation { field, expected } => {
                write!(f, "field `{field}` must be {expected} (§5.2)")
            }
        }
    }
}

impl std::error::Error for ManifestRejection {}

impl Manifest {
    /// Parse and validate a `plugin.json` document.
    ///
    /// On success, the accompanying diagnostics record any unknown top-level
    /// fields and a non-object `extensions` — reported and ignored. Every
    /// other schema violation rejects the manifest outright.
    pub fn parse(bytes: &[u8]) -> Result<(Self, Vec<Diagnostic>), ManifestRejection> {
        let document: Value = serde_json::from_slice(bytes)
            .map_err(|e| ManifestRejection::NotJson { detail: e.to_string() })?;
        let Value::Object(object) = document else {
            return Err(ManifestRejection::NotAnObject);
        };

        // Select validation rules from `$schema` before anything else.
        let spec = match object.get("$schema") {
            Some(Value::String(id)) if id == PLUGIN_SCHEMA_1_0_0 => SpecVersion::V1_0_0,
            Some(Value::String(other)) => {
                return Err(ManifestRejection::UnsupportedSchema { declared: other.clone() });
            }
            _ => return Err(ManifestRejection::MissingSchema),
        };

        let name = match object.get("name") {
            Some(Value::String(raw)) => {
                PluginName::parse(raw).map_err(ManifestRejection::InvalidName)?
            }
            _ => return Err(ManifestRejection::MissingName),
        };

        let mut diagnostics = Vec::new();
        let mut manifest = Manifest {
            spec,
            name,
            version: None,
            description: None,
            author: None,
            homepage: None,
            repository: None,
            license: None,
            keywords: Vec::new(),
            extensions: Extensions::default(),
        };

        for (field, value) in &object {
            match field.as_str() {
                "$schema" | "name" => {}
                "version" => manifest.version = Some(expect_string(field, value)?),
                "description" => manifest.description = Some(expect_string(field, value)?),
                "homepage" => manifest.homepage = Some(expect_string(field, value)?),
                "repository" => manifest.repository = Some(expect_string(field, value)?),
                "license" => manifest.license = Some(expect_string(field, value)?),
                "author" => manifest.author = Some(parse_author(value)?),
                "keywords" => manifest.keywords = parse_keywords(value)?,
                "extensions" => match value {
                    Value::Object(entries) => {
                        // The schema is closed below the top level too: a
                        // non-object namespace value is a fatal violation.
                        let mut extensions = BTreeMap::new();
                        for (ns, data) in entries {
                            if !data.is_object() {
                                return Err(ManifestRejection::SchemaViolation {
                                    field: format!("extensions.{ns}"),
                                    expected: "an object",
                                });
                            }
                            let text = serde_json::to_string(data)
                                .expect("a parsed JSON value serializes");
                            extensions.insert(ns.clone(), text);
                        }
                        manifest.extensions = Extensions { entries: extensions };
                    }
                    _ => diagnostics.push(Diagnostic::new(
                        Rule::ExtensionsIgnored,
                        Origin::Manifest,
                        "`extensions` is not an object; field ignored (§8.1)",
                    )),
                },
                unknown => diagnostics.push(Diagnostic::new(
                    Rule::UnknownManifestField,
                    Origin::Manifest,
                    format!("unknown top-level field `{unknown}` ignored (§5.2)"),
                )),
            }
        }

        Ok((manifest, diagnostics))
    }
}

fn expect_string(field: &str, value: &Value) -> Result<String, ManifestRejection> {
    match value {
        Value::String(s) => Ok(s.clone()),
        _ => Err(ManifestRejection::SchemaViolation { field: field.to_owned(), expected: "a string" }),
    }
}

fn parse_author(value: &Value) -> Result<Author, ManifestRejection> {
    let Value::Object(fields) = value else {
        return Err(ManifestRejection::SchemaViolation {
            field: "author".to_owned(),
            expected: "an object",
        });
    };
    let mut author = Author::default();
    for (field, value) in fields {
        let slot = match field.as_str() {
            "name" => &mut author.name,
            "email" => &mut author.email,
            "url" => &mut author.url,
            // The author object is closed; any other field is fatal.
            other => {
                return Err(ManifestRejection::SchemaViolation {
                    field: format!("author.{other}"),
                    expected: "absent (only `name`, `email`, `url` are permitted)",
                });
            }
        };
        *slot = Some(expect_string(&format!("author.{field}"), value)?);
    }
    Ok(author)
}

fn parse_keywords(value: &Value) -> Result<Vec<String>, ManifestRejection> {
    let Value::Array(items) = value else {
        return Err(ManifestRejection::SchemaViolation {
            field: "keywords".to_owned(),
            expected: "an array of strings",
        });
    };
    items.iter().map(|item| expect_string("keywords[]", item)).collect()
}

/// One plugin: a directory holding a `plugin.json`, and everything it declares.
///
/// The specification fixes the identity as the directory: the id is the
/// directory's name and the root is that directory, canonical.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Plugin {
    /// The plugin root's name, which the specification fixes as the id.
    pub id: String,
    /// The plugin root, canonical.
    pub root: PathBuf,
    /// The validated manifest.
    pub manifest: Manifest,
    /// Valid skills, in discovery order.
    pub skills: Vec<skill::Skill>,
    /// The MCP component's fate.
    pub mcp: McpOutcome,
    /// Everything non-fatal the loader decided along the way.
    pub diagnostics: Vec<Diagnostic>,
}

/// What became of the MCP component type.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum McpOutcome {
    /// No `mcp.json` — expressly not an error.
    Absent,
    /// MCP is disabled for this package; other components loaded on.
    Disabled(mcp::DisabledReason),
    /// Valid configuration; individually invalid entries were skipped.
    Configured(mcp::Config),
}

/// The package was rejected outright — nothing may be discovered or executed.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Rejection {
    /// The plugin root's directory name is not text, so it has no id.
    IdNotText,
    /// No `plugin.json` at the package root.
    ManifestMissing,
    /// `plugin.json` is not a regular file.
    ManifestNotAFile,
    /// `plugin.json` resolves outside the package root.
    ManifestEscapes,
    /// The manifest failed validation fatally.
    Manifest(ManifestRejection),
    /// The IO layer failed while the manifest was still in question.
    Source {
        /// The IO layer's explanation.
        detail: String,
    },
}

impl fmt::Display for Rejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IdNotText => f.write_str("the plugin root's name is not text"),
            Self::ManifestMissing => f.write_str("no plugin.json at the package root (§5.1)"),
            Self::ManifestNotAFile => f.write_str("plugin.json is not a regular file"),
            Self::ManifestEscapes => {
                f.write_str("plugin.json resolves outside the package root (§4.1)")
            }
            Self::Manifest(rejection) => write!(f, "manifest rejected: {rejection}"),
            Self::Source { detail } => write!(f, "IO failed: {detail}"),
        }
    }
}

impl std::error::Error for Rejection {}

/// Load the plugin rooted at `root` from the filesystem.
///
/// The package root is the directory the specification calls a plugin root —
/// the `.agents/` directory by convention. The manifest is read first; only
/// once it validates are `skills/` and `mcp.json` discovered.
pub async fn load(root: impl AsRef<Path>) -> Result<Plugin, Rejection> {
    let root = root.as_ref();
    // The id is the root's own name, so a root that has none, or one that is
    // not text, cannot be the plugin the caller is asking for.
    let id = root
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(Rejection::IdNotText)?
        .to_owned();
    let canonical_root = tokio::fs::canonicalize(root).await.map_err(|error| Rejection::Source {
        detail: format!("cannot open package root: {error}"),
    })?;

    let (manifest, mut diagnostics) = load_manifest(root, &canonical_root).await?;

    let (skills, skill_diagnostics) = load_skills(root, &canonical_root).await;
    let (mcp, mcp_diagnostics) = load_mcp(root, &canonical_root, manifest.spec).await;
    diagnostics.extend(skill_diagnostics);
    diagnostics.extend(mcp_diagnostics);

    Ok(Plugin { id, root: canonical_root, manifest, skills, mcp, diagnostics })
}

async fn load_manifest(root: &Path, canonical_root: &Path) -> Result<(Manifest, Vec<Diagnostic>), Rejection> {
    let path = root.join("plugin.json");
    match file_kind(&path).await {
        Ok(Some(FileKind::File)) => {}
        Ok(None) => return Err(Rejection::ManifestMissing),
        Ok(Some(_)) => return Err(Rejection::ManifestNotAFile),
        Err(error) => return Err(Rejection::Source { detail: error.to_string() }),
    }
    match confined(&path, canonical_root).await {
        Ok(true) => {}
        Ok(false) => return Err(Rejection::ManifestEscapes),
        Err(error) => return Err(Rejection::Source { detail: error.to_string() }),
    }
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|error| Rejection::Source { detail: error.to_string() })?;
    let (manifest, diagnostics) = Manifest::parse(&bytes).map_err(Rejection::Manifest)?;
    Ok((manifest, diagnostics))
}

pub(crate) async fn load_skills(
    root: &Path,
    canonical_root: &Path,
) -> (Vec<skill::Skill>, Vec<Diagnostic>) {
    let mut diagnostics = Vec::new();
    let dir = root.join("skills");
    match file_kind(&dir).await {
        Ok(Some(FileKind::Directory)) => {}
        // An absent component location is not an error.
        Ok(None) => return (Vec::new(), diagnostics),
        Ok(Some(_)) => {
            diagnostics.push(component_invalid(
                Origin::Skills,
                "`skills` exists but is not a directory (§6.2)",
            ));
            return (Vec::new(), diagnostics);
        }
        Err(error) => {
            diagnostics.push(component_invalid(Origin::Skills, error.to_string()));
            return (Vec::new(), diagnostics);
        }
    }
    match confined(&dir, canonical_root).await {
        Ok(true) => {}
        Ok(false) => {
            diagnostics.push(component_invalid(
                Origin::Skills,
                "`skills` resolves outside the package root (§4.1)",
            ));
            return (Vec::new(), diagnostics);
        }
        Err(error) => {
            diagnostics.push(component_invalid(Origin::Skills, error.to_string()));
            return (Vec::new(), diagnostics);
        }
    }

    let mut entries = match tokio::fs::read_dir(&dir).await {
        Ok(entries) => entries,
        Err(error) => {
            diagnostics.push(component_invalid(Origin::Skills, error.to_string()));
            return (Vec::new(), diagnostics);
        }
    };
    // Only an immediate child *directory* may hold a skill; files and deeper
    // descendants are never searched.
    let mut directories: Vec<(String, PathBuf)> = Vec::new();
    loop {
        let entry = match entries.next_entry().await {
            Ok(Some(entry)) => entry,
            Ok(None) => break,
            Err(error) => {
                diagnostics.push(component_invalid(Origin::Skills, error.to_string()));
                break;
            }
        };
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        // Follow symlinks so a linked directory counts as one.
        if matches!(file_kind(&entry.path()).await, Ok(Some(FileKind::Directory))) {
            directories.push((name, entry.path()));
        }
    }
    directories.sort_by(|left, right| left.0.cmp(&right.0));

    let mut skills = Vec::new();
    for (name, directory) in directories {
        let file = directory.join("SKILL.md");
        match file_kind(&file).await {
            // No SKILL.md, or not a regular file: simply not a skill.
            Ok(Some(FileKind::File)) => {}
            Ok(_) => continue,
            Err(error) => {
                diagnostics.push(skill_skipped(&name, error.to_string()));
                continue;
            }
        }
        match confined(&file, canonical_root).await {
            Ok(true) => {}
            Ok(false) => {
                diagnostics.push(skill_skipped(
                    &name,
                    "SKILL.md resolves outside the package root (§4.1)".to_owned(),
                ));
                continue;
            }
            Err(error) => {
                diagnostics.push(skill_skipped(&name, error.to_string()));
                continue;
            }
        }
        let bytes = match tokio::fs::read(&file).await {
            Ok(bytes) => bytes,
            Err(error) => {
                diagnostics.push(skill_skipped(&name, error.to_string()));
                continue;
            }
        };
        match skill::parse_skill_md(&name, &bytes) {
            Ok((meta, body)) => skills.push(skill::Skill {
                directory: name.clone(),
                path: PackagePath::new("skills").child(&name),
                meta,
                body,
            }),
            Err(why) => diagnostics.push(skill_skipped(&name, why.to_string())),
        }
    }
    (skills, diagnostics)
}

pub(crate) async fn load_mcp(
    root: &Path,
    canonical_root: &Path,
    spec: SpecVersion,
) -> (McpOutcome, Vec<Diagnostic>) {
    let path = root.join("mcp.json");
    match file_kind(&path).await {
        Ok(Some(FileKind::File)) => {}
        Ok(None) => return (McpOutcome::Absent, Vec::new()),
        Ok(Some(_)) => {
            let diagnostic = component_invalid(
                Origin::Mcp,
                "`mcp.json` exists but is not a regular file (§6.2)",
            );
            return (McpOutcome::Disabled(mcp::DisabledReason::NotAFile), vec![diagnostic]);
        }
        Err(error) => {
            return disabled(mcp::DisabledReason::Unavailable { detail: error.to_string() });
        }
    }
    match confined(&path, canonical_root).await {
        Ok(true) => {}
        Ok(false) => {
            let diagnostic = component_invalid(
                Origin::Mcp,
                "`mcp.json` resolves outside the package root (§4.1)",
            );
            return (McpOutcome::Disabled(mcp::DisabledReason::LocationEscapes), vec![diagnostic]);
        }
        Err(error) => {
            return disabled(mcp::DisabledReason::Unavailable { detail: error.to_string() });
        }
    }
    let bytes = match tokio::fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(error) => {
            return disabled(mcp::DisabledReason::Unavailable { detail: error.to_string() });
        }
    };
    match mcp::Config::parse(&bytes, spec) {
        Ok((config, server_diagnostics)) => (McpOutcome::Configured(config), server_diagnostics),
        Err(reason) => disabled(reason),
    }
}

/// MCP is disabled for the package: the outcome, and the report that says so.
fn disabled(reason: mcp::DisabledReason) -> (McpOutcome, Vec<Diagnostic>) {
    let diagnostic = Diagnostic::new(Rule::McpDisabled, Origin::Mcp, reason.to_string());
    (McpOutcome::Disabled(reason), vec![diagnostic])
}

fn component_invalid(origin: Origin, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(Rule::ComponentInvalid, origin, message)
}

fn skill_skipped(directory: &str, why: String) -> Diagnostic {
    Diagnostic::new(
        Rule::SkillSkipped,
        Origin::Skill(directory.to_owned()),
        format!("skill skipped (§7.1): {why}"),
    )
}

/// What kind of entry a path resolves to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileKind {
    /// A regular file (or a symlink resolving to one).
    File,
    /// A directory (or a symlink resolving to one).
    Directory,
    /// Anything else — sockets, devices, dangling links.
    Other,
}

async fn file_kind(path: &Path) -> io::Result<Option<FileKind>> {
    match tokio::fs::metadata(path).await {
        Ok(meta) if meta.is_file() => Ok(Some(FileKind::File)),
        Ok(meta) if meta.is_dir() => Ok(Some(FileKind::Directory)),
        Ok(_) => Ok(Some(FileKind::Other)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Whether `path`, fully resolved, stays within the package root.
async fn confined(path: &Path, canonical_root: &Path) -> io::Result<bool> {
    let resolved = tokio::fs::canonicalize(path).await?;
    Ok(resolved.starts_with(canonical_root))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    fn parse(json: &str) -> Result<(Manifest, Vec<Diagnostic>), ManifestRejection> {
        Manifest::parse(json.as_bytes())
    }

    #[test]
    fn minimal_manifest() {
        let (manifest, diags) = parse(
            r#"{"$schema": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
                "name": "minimal-plugin"}"#,
        )
        .unwrap();
        assert_eq!(manifest.name, *"minimal-plugin");
        assert_eq!(manifest.spec, SpecVersion::V1_0_0);
        assert!(diags.is_empty());
    }

    #[test]
    fn unknown_field_is_reported_and_ignored() {
        let (manifest, diags) = parse(
            r#"{"$schema": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
                "name": "p", "hooks": {}}"#,
        )
        .unwrap();
        assert_eq!(manifest.name, *"p");
        assert_eq!(diags[0].rule, Rule::UnknownManifestField);
    }

    #[test]
    fn non_object_extensions_is_reported_and_ignored() {
        let (manifest, diags) = parse(
            r#"{"$schema": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
                "name": "p", "extensions": 7}"#,
        )
        .unwrap();
        assert!(manifest.extensions.is_empty());
        assert_eq!(diags[0].rule, Rule::ExtensionsIgnored);
    }

    #[test]
    fn non_object_extension_namespace_is_fatal() {
        let err = parse(
            r#"{"$schema": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
                "name": "p", "extensions": {"com.example.client": 7}}"#,
        )
        .unwrap_err();
        assert!(matches!(err, ManifestRejection::SchemaViolation { .. }));
    }

    #[test]
    fn extension_data_is_carried_verbatim_and_hashable() {
        let (manifest, _) = parse(
            r#"{"$schema": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
                "name": "p", "extensions": {"com.example.client": {"setting": true}}}"#,
        )
        .unwrap();
        assert_eq!(manifest.extensions.raw("com.example.client"), Some(r#"{"setting":true}"#));
        assert!(manifest.extensions.raw("org.other").is_none());
        // The whole manifest can key a map, which a `serde_json::Value` cannot.
        let mut seen = std::collections::HashSet::new();
        seen.insert(manifest);
    }

    #[test]
    fn unsupported_schema_is_its_own_rejection() {
        let err = parse(
            r#"{"$schema": "https://agent-plugins.org/schemas/9.0.0/plugin.schema.json",
                "name": "p"}"#,
        )
        .unwrap_err();
        assert!(matches!(err, ManifestRejection::UnsupportedSchema { .. }));
    }

    #[tokio::test]
    async fn loads_a_package_of_manifest_skills_and_mcp() {
        let root = TempDir::new().await;
        tokio::fs::create_dir_all(root.path().join("skills/greet")).await.unwrap();
        tokio::fs::write(root.path().join("plugin.json"), MANIFEST).await.unwrap();
        tokio::fs::write(
            root.path().join("skills/greet/SKILL.md"),
            b"---\nname: greet\ndescription: Greets. Use when greeting.\n---\nHi.\n",
        )
        .await
        .unwrap();
        tokio::fs::write(
            root.path().join("mcp.json"),
            format!(
                r#"{{"$schema": "{}",
                     "mcpServers": {{"echo": {{"type": "stdio", "command": "npx"}}}}}}"#,
                crate::mcp::MCP_SCHEMA_1_0_0
            ),
        )
        .await
        .unwrap();

        let plugin = load(root.path()).await.unwrap();
        assert_eq!(plugin.manifest.name, *"demo");
        assert_eq!(plugin.skills.len(), 1);
        assert_eq!(plugin.skills[0].meta.name, "greet");
        assert_eq!(plugin.skills[0].path.as_str(), "skills/greet");
        assert_eq!(
            plugin.mcp,
            McpOutcome::Configured(mcp::Config {
                servers: vec![mcp::ServerEntry {
                    name: "echo".to_owned(),
                    server: crate::mcp::Server::Stdio(crate::mcp::StdioServer {
                        command: crate::mcp::Command::Bare("npx".to_owned()),
                        args: Vec::new(),
                        env: Vec::new(),
                        cwd: crate::mcp::Cwd::PluginRoot,
                    }),
                }],
            })
        );
    }

    #[tokio::test]
    async fn a_missing_manifest_rejects_the_package() {
        let root = TempDir::new().await;
        assert_eq!(load(root.path()).await, Err(Rejection::ManifestMissing));
    }

    const MANIFEST: &[u8] = br#"{
        "$schema": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
        "name": "demo"
    }"#;

    /// A scratch directory removed when the test ends.
    struct TempDir(PathBuf);

    impl TempDir {
        async fn new() -> Self {
            // A counter, not the clock: two tests starting in the same tick must
            // not share a path, or one's cleanup deletes the other's tree.
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let id = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("dotagents-test-{}-{id}", std::process::id()));
            tokio::fs::create_dir_all(&path).await.unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
