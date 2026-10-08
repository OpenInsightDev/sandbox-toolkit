//! The plugin specification defers the skill *format* to the Agent Skills
//! specification, so this module implements that format's validation. Skipping
//! is per-skill and non-fatal: the reason travels in a diagnostic and loading
//! continues.

mod markdown;

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::path::Path;

use yaml_rust2::{Yaml, YamlLoader};

pub use markdown::{Document, HeadingLevel, Link, LinkTarget, Section, Slug};

use crate::path::PackagePath;

/// One valid skill discovered under `skills/`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Skill {
    /// The immediate child directory of `skills/` this skill lives in; always
    /// equal to `meta.name`, as the Agent Skills specification requires the
    /// frontmatter name to match the directory name.
    pub directory: String,
    pub path: PackagePath,
    pub meta: Meta,
    /// Parsed into a navigable [`Document`] so a client can address one section
    /// instead of the whole body.
    pub body: Document,
}

/// Validated `SKILL.md` frontmatter per the Agent Skills specification.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Meta {
    /// 1–64 lowercase alphanumeric characters and hyphens, matching the parent
    /// directory name.
    pub name: String,
    /// What the skill does and when to use it (1–1024 characters).
    pub description: String,
    /// License name or reference to a bundled license file.
    pub license: Option<String>,
    /// Environment requirements (1–500 characters).
    pub compatibility: Option<String>,
    /// Space-separated pre-approved tools (experimental).
    pub allowed_tools: Option<String>,
    pub metadata: BTreeMap<String, String>,
}

/// Why a discovered skill was skipped.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Invalid {
    NotUtf8,
    NoFrontmatter,
    UnterminatedFrontmatter,
    BadYaml { detail: String },
    NotAMapping,
    UnknownField { field: String },
    MissingField {
        /// `name` or `description`.
        field: &'static str,
    },
    WrongType {
        field: String,
        /// What the Agent Skills specification requires there.
        expected: &'static str,
    },
    BadName { reason: String },
    NameMismatch { name: String, directory: String },
    BadDescription { reason: String },
    BadCompatibility { reason: String },
}

impl fmt::Display for Invalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotUtf8 => f.write_str("SKILL.md is not UTF-8 text"),
            Self::NoFrontmatter => {
                f.write_str("SKILL.md must begin with YAML frontmatter (`---`)")
            }
            Self::UnterminatedFrontmatter => f.write_str("SKILL.md frontmatter is not closed with `---`"),
            Self::BadYaml { detail } => write!(f, "invalid YAML in frontmatter: {detail}"),
            Self::NotAMapping => f.write_str("frontmatter must be a YAML mapping"),
            Self::UnknownField { field } => write!(f, "unexpected frontmatter field `{field}`"),
            Self::MissingField { field } => {
                write!(f, "missing required frontmatter field `{field}`")
            }
            Self::WrongType { field, expected } => {
                write!(f, "frontmatter field `{field}` must be {expected}")
            }
            Self::BadName { reason } => write!(f, "invalid skill name: {reason}"),
            Self::NameMismatch { name, directory } => {
                write!(f, "frontmatter name {name:?} must match directory name {directory:?}")
            }
            Self::BadDescription { reason } => write!(f, "invalid description: {reason}"),
            Self::BadCompatibility { reason } => write!(f, "invalid compatibility: {reason}"),
        }
    }
}

impl std::error::Error for Invalid {}

/// Why a skill directory could not be loaded.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum LoadError {
    /// No `SKILL.md`, or it is not a regular file: simply not a skill.
    Missing,
    Unavailable { detail: String },
    /// The path has no usable directory name to match the frontmatter against.
    Unnamed,
    Skipped(Invalid),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => f.write_str("no SKILL.md in the skill directory"),
            Self::Unavailable { detail } => write!(f, "SKILL.md could not be read: {detail}"),
            Self::Unnamed => f.write_str("skill directory has no usable name"),
            Self::Skipped(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for LoadError {}

pub async fn load(dir: impl AsRef<Path>) -> Result<Skill, LoadError> {
    let dir = dir.as_ref();
    let Some(directory) = dir.file_name().and_then(|name| name.to_str()) else {
        return Err(LoadError::Unnamed);
    };
    let file = dir.join("SKILL.md");
    match tokio::fs::metadata(&file).await {
        Ok(meta) if meta.is_file() => {}
        Ok(_) => return Err(LoadError::Missing),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Err(LoadError::Missing),
        Err(error) => return Err(LoadError::Unavailable { detail: error.to_string() }),
    }
    let bytes = tokio::fs::read(&file)
        .await
        .map_err(|error| LoadError::Unavailable { detail: error.to_string() })?;
    let (meta, body) = parse_skill_md(directory, &bytes).map_err(LoadError::Skipped)?;
    Ok(Skill {
        directory: directory.to_owned(),
        path: PackagePath::new("skills").child(directory),
        meta,
        body,
    })
}

pub fn parse_skill_md(
    directory: &str,
    bytes: &[u8],
) -> Result<(Meta, Document), Invalid> {
    let text = std::str::from_utf8(bytes).map_err(|_| Invalid::NotUtf8)?;
    let (frontmatter, body) = split_frontmatter(text)?;

    let docs = YamlLoader::load_from_str(frontmatter)
        .map_err(|e| Invalid::BadYaml { detail: e.to_string() })?;
    let mapping = match docs.first() {
        Some(Yaml::Hash(mapping)) => mapping,
        Some(_) | None => return Err(Invalid::NotAMapping),
    };

    let mut name = None;
    let mut description = None;
    let mut license = None;
    let mut compatibility = None;
    let mut allowed_tools = None;
    let mut metadata = BTreeMap::new();

    for (key, value) in mapping {
        let Yaml::String(key) = key else {
            return Err(Invalid::NotAMapping);
        };
        match key.as_str() {
            "name" => name = Some(expect_str(key, value)?),
            "description" => description = Some(expect_str(key, value)?),
            "license" => license = Some(expect_str(key, value)?),
            "compatibility" => compatibility = Some(expect_str(key, value)?),
            "allowed-tools" => allowed_tools = Some(expect_str(key, value)?),
            "metadata" => {
                let Yaml::Hash(entries) = value else {
                    return Err(Invalid::WrongType {
                        field: "metadata".to_owned(),
                        expected: "a mapping of strings to strings",
                    });
                };
                for (k, v) in entries {
                    let (Some(k), Some(v)) = (scalar_to_string(k), scalar_to_string(v)) else {
                        return Err(Invalid::WrongType {
                            field: "metadata".to_owned(),
                            expected: "a mapping of strings to strings",
                        });
                    };
                    metadata.insert(k, v);
                }
            }
            unknown => return Err(Invalid::UnknownField { field: unknown.to_owned() }),
        }
    }

    let name = name.ok_or(Invalid::MissingField { field: "name" })?;
    let description = description.ok_or(Invalid::MissingField { field: "description" })?;

    validate_name(&name)?;
    if name != directory {
        return Err(Invalid::NameMismatch { name, directory: directory.to_owned() });
    }
    validate_description(&description)?;
    if let Some(compat) = &compatibility {
        let count = compat.chars().count();
        if compat.trim().is_empty() {
            return Err(Invalid::BadCompatibility { reason: "empty".to_owned() });
        }
        if count > 500 {
            return Err(Invalid::BadCompatibility {
                reason: format!("{count} characters; the limit is 500"),
            });
        }
    }

    Ok((
        Meta { name, description, license, compatibility, allowed_tools, metadata },
        Document::parse(body),
    ))
}

fn split_frontmatter(text: &str) -> Result<(&str, &str), Invalid> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.split_inclusive('\n');
    let first = lines.next().unwrap_or("");
    if first.trim_end() != "---" {
        return Err(Invalid::NoFrontmatter);
    }
    let after_open = &text[first.len()..];
    let mut offset = 0;
    for line in after_open.split_inclusive('\n') {
        if line.trim_end() == "---" {
            let frontmatter = &after_open[..offset];
            let body = &after_open[offset + line.len()..];
            return Ok((frontmatter, body.trim_start_matches('\n')));
        }
        offset += line.len();
    }
    Err(Invalid::UnterminatedFrontmatter)
}

fn expect_str(field: &str, value: &Yaml) -> Result<String, Invalid> {
    match value {
        Yaml::String(s) => Ok(s.clone()),
        _ => Err(Invalid::WrongType { field: field.to_owned(), expected: "a string" }),
    }
}

/// Frontmatter `metadata` values are coerced from scalars, mirroring the
/// reference implementation's tolerance for `version: "1.0"` vs `version: 1.0`.
fn scalar_to_string(value: &Yaml) -> Option<String> {
    match value {
        Yaml::String(s) => Some(s.clone()),
        Yaml::Integer(i) => Some(i.to_string()),
        Yaml::Real(r) => Some(r.clone()),
        Yaml::Boolean(b) => Some(b.to_string()),
        _ => None,
    }
}

fn validate_name(name: &str) -> Result<(), Invalid> {
    let bad = |reason: &str| Invalid::BadName { reason: reason.to_owned() };
    if name.trim().is_empty() {
        return Err(bad("empty"));
    }
    if name.chars().count() > 64 {
        return Err(bad("longer than 64 characters"));
    }
    if name.chars().any(|c| c.is_uppercase()) {
        return Err(bad("must be lowercase"));
    }
    if !name.chars().all(|c| c.is_alphanumeric() || c == '-') {
        return Err(bad("only letters, digits, and hyphens are allowed"));
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Err(bad("must not start or end with a hyphen"));
    }
    if name.contains("--") {
        return Err(bad("must not contain consecutive hyphens"));
    }
    Ok(())
}

fn validate_description(description: &str) -> Result<(), Invalid> {
    if description.trim().is_empty() {
        return Err(Invalid::BadDescription { reason: "empty".to_owned() });
    }
    let count = description.chars().count();
    if count > 1024 {
        return Err(Invalid::BadDescription {
            reason: format!("{count} characters; the limit is 1024"),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str =
        "---\nname: deploy\ndescription: Deploys things. Use when deploying.\n---\n\nBody here.\n";

    #[test]
    fn valid_skill() {
        let (meta, body) = parse_skill_md("deploy", VALID.as_bytes()).unwrap();
        assert_eq!(meta.name, "deploy");
        assert_eq!(body.source(), "Body here.\n");
    }

    #[test]
    fn name_must_match_directory() {
        let err = parse_skill_md("other", VALID.as_bytes()).unwrap_err();
        assert!(matches!(err, Invalid::NameMismatch { .. }));
    }

    #[test]
    fn frontmatter_is_required() {
        assert_eq!(parse_skill_md("x", b"# Just markdown"), Err(Invalid::NoFrontmatter));
        assert_eq!(
            parse_skill_md("x", b"---\nname: x\n"),
            Err(Invalid::UnterminatedFrontmatter)
        );
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let text = "---\nname: x\ndescription: d\nversion: 1.0\n---\n";
        assert_eq!(
            parse_skill_md("x", text.as_bytes()),
            Err(Invalid::UnknownField { field: "version".to_owned() })
        );
    }

    #[test]
    fn metadata_scalars_coerce() {
        let text = "---\nname: x\ndescription: d\nmetadata:\n  version: 1.0\n  pinned: true\n---\n";
        let (meta, _) = parse_skill_md("x", text.as_bytes()).unwrap();
        assert_eq!(meta.metadata.get("version").map(String::as_str), Some("1.0"));
        assert_eq!(meta.metadata.get("pinned").map(String::as_str), Some("true"));
    }

    #[tokio::test]
    async fn load_reads_a_skill_directory() {
        let root = std::env::temp_dir().join(format!(
            "dotagents-skill-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("the clock is past the epoch")
                .as_nanos()
        ));
        let directory = root.join("greet");
        tokio::fs::create_dir_all(&directory).await.unwrap();

        // A directory without a `SKILL.md` is simply not a skill.
        assert_eq!(load(&directory).await, Err(LoadError::Missing));

        tokio::fs::write(
            directory.join("SKILL.md"),
            b"---\nname: greet\ndescription: Greets. Use when greeting.\n---\nBody.\n",
        )
        .await
        .unwrap();
        let skill = load(&directory).await.unwrap();
        assert_eq!(skill.directory, "greet");
        assert_eq!(skill.path.as_str(), "skills/greet");
        assert_eq!(skill.body.source(), "Body.\n");

        let _ = tokio::fs::remove_dir_all(&root).await;
    }
}
