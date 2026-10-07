//! Agent Skills, as the specification discovers them.
//!
//! The plugin specification defers the skill *format* to the Agent Skills
//! specification; this module implements that format's validation, so a
//! discovered skill can be accepted or skipped with a precise reason.
//! Skipping is per-skill and non-fatal: the reason travels in a diagnostic and
//! loading continues.

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
    /// The immediate child directory of `skills/` this skill lives in.
    /// Always equal to `meta.name` — the Agent Skills specification requires
    /// the frontmatter name to match the directory name.
    pub directory: String,
    /// Package path of the skill directory (`skills/<directory>`).
    pub path: PackagePath,
    /// Validated frontmatter.
    pub meta: SkillMeta,
    /// The instructions after the frontmatter, parsed into a navigable
    /// [`Document`] so a client can address one section instead of the whole
    /// body. [`Document::source`] returns the original Markdown.
    pub body: Document,
}

/// Validated `SKILL.md` frontmatter per the Agent Skills specification.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct SkillMeta {
    /// Skill name: 1–64 lowercase alphanumeric characters and hyphens,
    /// matching the parent directory name.
    pub name: String,
    /// What the skill does and when to use it (1–1024 characters).
    pub description: String,
    /// License name or reference to a bundled license file.
    pub license: Option<String>,
    /// Environment requirements (1–500 characters).
    pub compatibility: Option<String>,
    /// Space-separated pre-approved tools (experimental).
    pub allowed_tools: Option<String>,
    /// Arbitrary string-to-string metadata.
    pub metadata: BTreeMap<String, String>,
}

/// Why a discovered skill was skipped.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SkillInvalid {
    /// `SKILL.md` is not UTF-8 text.
    NotUtf8,
    /// The file does not begin with a `---` frontmatter fence.
    NoFrontmatter,
    /// The opening fence is never closed by a `---` line.
    UnterminatedFrontmatter,
    /// The frontmatter is not parseable YAML.
    BadYaml {
        /// The YAML parser's explanation.
        detail: String,
    },
    /// The frontmatter is valid YAML but not a mapping.
    NotAMapping,
    /// A frontmatter key outside the Agent Skills specification.
    UnknownField {
        /// The unexpected key.
        field: String,
    },
    /// A required field is missing.
    MissingField {
        /// The missing key: `name` or `description`.
        field: &'static str,
    },
    /// A field has the wrong YAML type.
    WrongType {
        /// The offending key.
        field: String,
        /// What the Agent Skills specification requires there.
        expected: &'static str,
    },
    /// The `name` field breaks a naming constraint.
    BadName {
        /// Which constraint broke.
        reason: String,
    },
    /// The frontmatter name does not match the directory name.
    NameMismatch {
        /// The name declared in frontmatter.
        name: String,
        /// The directory the skill was discovered in.
        directory: String,
    },
    /// The `description` is empty or over 1024 characters.
    BadDescription {
        /// Which constraint broke.
        reason: String,
    },
    /// The `compatibility` field is empty or over 500 characters.
    BadCompatibility {
        /// Which constraint broke.
        reason: String,
    },
}

impl fmt::Display for SkillInvalid {
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

impl std::error::Error for SkillInvalid {}

/// Why a skill directory could not be loaded.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SkillLoadError {
    /// No `SKILL.md`, or it is not a regular file: simply not a skill.
    Missing,
    /// `SKILL.md` could not be read.
    Unavailable {
        /// The IO layer's explanation.
        detail: String,
    },
    /// The path has no usable directory name to match the frontmatter against.
    Unnamed,
    /// `SKILL.md` is present but invalid.
    Invalid(SkillInvalid),
}

impl fmt::Display for SkillLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => f.write_str("no SKILL.md in the skill directory"),
            Self::Unavailable { detail } => write!(f, "SKILL.md could not be read: {detail}"),
            Self::Unnamed => f.write_str("skill directory has no usable name"),
            Self::Invalid(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for SkillLoadError {}

/// Read the `SKILL.md` in the skill directory at `dir`.
///
/// The directory name is the skill's name, and `path` is reported at the fixed
/// package location `skills/<name>`.
pub async fn load(dir: impl AsRef<Path>) -> Result<Skill, SkillLoadError> {
    let dir = dir.as_ref();
    let Some(directory) = dir.file_name().and_then(|name| name.to_str()) else {
        return Err(SkillLoadError::Unnamed);
    };
    let file = dir.join("SKILL.md");
    match tokio::fs::metadata(&file).await {
        Ok(meta) if meta.is_file() => {}
        Ok(_) => return Err(SkillLoadError::Missing),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Err(SkillLoadError::Missing),
        Err(error) => return Err(SkillLoadError::Unavailable { detail: error.to_string() }),
    }
    let bytes = tokio::fs::read(&file)
        .await
        .map_err(|error| SkillLoadError::Unavailable { detail: error.to_string() })?;
    let (meta, body) = parse_skill_md(directory, &bytes).map_err(SkillLoadError::Invalid)?;
    Ok(Skill {
        directory: directory.to_owned(),
        path: PackagePath::new("skills").child(directory),
        meta,
        body,
    })
}

/// Parse and validate the contents of a `SKILL.md` found at
/// `skills/<directory>/SKILL.md`.
pub fn parse_skill_md(
    directory: &str,
    bytes: &[u8],
) -> Result<(SkillMeta, Document), SkillInvalid> {
    let text = std::str::from_utf8(bytes).map_err(|_| SkillInvalid::NotUtf8)?;
    let (frontmatter, body) = split_frontmatter(text)?;

    let docs = YamlLoader::load_from_str(frontmatter)
        .map_err(|e| SkillInvalid::BadYaml { detail: e.to_string() })?;
    let mapping = match docs.first() {
        Some(Yaml::Hash(mapping)) => mapping,
        Some(_) | None => return Err(SkillInvalid::NotAMapping),
    };

    let mut name = None;
    let mut description = None;
    let mut license = None;
    let mut compatibility = None;
    let mut allowed_tools = None;
    let mut metadata = BTreeMap::new();

    for (key, value) in mapping {
        let Yaml::String(key) = key else {
            return Err(SkillInvalid::NotAMapping);
        };
        match key.as_str() {
            "name" => name = Some(expect_str(key, value)?),
            "description" => description = Some(expect_str(key, value)?),
            "license" => license = Some(expect_str(key, value)?),
            "compatibility" => compatibility = Some(expect_str(key, value)?),
            "allowed-tools" => allowed_tools = Some(expect_str(key, value)?),
            "metadata" => {
                let Yaml::Hash(entries) = value else {
                    return Err(SkillInvalid::WrongType {
                        field: "metadata".to_owned(),
                        expected: "a mapping of strings to strings",
                    });
                };
                for (k, v) in entries {
                    let (Some(k), Some(v)) = (scalar_to_string(k), scalar_to_string(v)) else {
                        return Err(SkillInvalid::WrongType {
                            field: "metadata".to_owned(),
                            expected: "a mapping of strings to strings",
                        });
                    };
                    metadata.insert(k, v);
                }
            }
            unknown => return Err(SkillInvalid::UnknownField { field: unknown.to_owned() }),
        }
    }

    let name = name.ok_or(SkillInvalid::MissingField { field: "name" })?;
    let description = description.ok_or(SkillInvalid::MissingField { field: "description" })?;

    validate_name(&name)?;
    if name != directory {
        return Err(SkillInvalid::NameMismatch { name, directory: directory.to_owned() });
    }
    validate_description(&description)?;
    if let Some(compat) = &compatibility {
        let count = compat.chars().count();
        if compat.trim().is_empty() {
            return Err(SkillInvalid::BadCompatibility { reason: "empty".to_owned() });
        }
        if count > 500 {
            return Err(SkillInvalid::BadCompatibility {
                reason: format!("{count} characters; the limit is 500"),
            });
        }
    }

    Ok((
        SkillMeta { name, description, license, compatibility, allowed_tools, metadata },
        Document::parse(body),
    ))
}

/// Split `---`-fenced YAML frontmatter from the Markdown body.
fn split_frontmatter(text: &str) -> Result<(&str, &str), SkillInvalid> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.split_inclusive('\n');
    let first = lines.next().unwrap_or("");
    if first.trim_end() != "---" {
        return Err(SkillInvalid::NoFrontmatter);
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
    Err(SkillInvalid::UnterminatedFrontmatter)
}

fn expect_str(field: &str, value: &Yaml) -> Result<String, SkillInvalid> {
    match value {
        Yaml::String(s) => Ok(s.clone()),
        _ => Err(SkillInvalid::WrongType { field: field.to_owned(), expected: "a string" }),
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

/// Agent Skills name rules: 1–64 characters, lowercase letters/digits/hyphens,
/// alphanumeric at the edges, no `--`.
fn validate_name(name: &str) -> Result<(), SkillInvalid> {
    let bad = |reason: &str| SkillInvalid::BadName { reason: reason.to_owned() };
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

fn validate_description(description: &str) -> Result<(), SkillInvalid> {
    if description.trim().is_empty() {
        return Err(SkillInvalid::BadDescription { reason: "empty".to_owned() });
    }
    let count = description.chars().count();
    if count > 1024 {
        return Err(SkillInvalid::BadDescription {
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
        assert!(matches!(err, SkillInvalid::NameMismatch { .. }));
    }

    #[test]
    fn frontmatter_is_required() {
        assert_eq!(parse_skill_md("x", b"# Just markdown"), Err(SkillInvalid::NoFrontmatter));
        assert_eq!(
            parse_skill_md("x", b"---\nname: x\n"),
            Err(SkillInvalid::UnterminatedFrontmatter)
        );
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let text = "---\nname: x\ndescription: d\nversion: 1.0\n---\n";
        assert_eq!(
            parse_skill_md("x", text.as_bytes()),
            Err(SkillInvalid::UnknownField { field: "version".to_owned() })
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
        assert_eq!(load(&directory).await, Err(SkillLoadError::Missing));

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
