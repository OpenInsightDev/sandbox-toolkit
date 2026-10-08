//! The specification demands "a single, non-recursive textual replacement" in
//! which text introduced by a replacement is never rescanned. Rather than
//! policing that with careful string code at expansion time, a [`Template`] is
//! parsed once into segments — literal runs and placeholder markers — and
//! expansion just concatenates. Non-recursion is a property of the shape, not
//! of discipline.

use std::fmt;
use std::path::{Path, PathBuf};

/// The two path anchors a client provides to a plugin subprocess.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Anchors {
    /// Absolute path of the filesystem-resolved package root (`PLUGIN_ROOT`).
    pub plugin_root: PathBuf,
    /// Absolute path of the client-managed persistent data directory
    /// dedicated to this installed package instance (`PLUGIN_DATA`).
    pub plugin_data: PathBuf,
}

impl Anchors {
    pub fn new(plugin_root: impl Into<PathBuf>, plugin_data: impl Into<PathBuf>) -> Self {
        Self { plugin_root: plugin_root.into(), plugin_data: plugin_data.into() }
    }

    fn path_of(&self, placeholder: Placeholder) -> &Path {
        match placeholder {
            Placeholder::PluginRoot => &self.plugin_root,
            Placeholder::PluginData => &self.plugin_data,
        }
    }
}

/// The two placeholders the specification defines — and the only two it
/// permits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Placeholder {
    PluginRoot,
    PluginData,
}

impl Placeholder {
    /// The literal spelling, braces and all.
    pub fn spelling(self) -> &'static str {
        match self {
            Self::PluginRoot => "${PLUGIN_ROOT}",
            Self::PluginData => "${PLUGIN_DATA}",
        }
    }
}

impl fmt::Display for Placeholder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.spelling())
    }
}

/// One parsed piece of a configured string.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Segment {
    /// Literal text, preserved exactly — including any placeholder-*like* text
    /// that is not one of the two recognized placeholders.
    Literal(String),
    Anchor(Placeholder),
}

/// A configured string from `args`, `env`, or `cwd`, parsed for expansion.
///
/// Parsing is infallible: anything that is not exactly `${PLUGIN_ROOT}` or
/// `${PLUGIN_DATA}` — including `$PLUGIN_ROOT`, `${plugin_root}`, `${HOME}` —
/// remains literal text, as the specification requires.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Template {
    raw: String,
    segments: Vec<Segment>,
}

impl Template {
    pub fn parse(raw: &str) -> Self {
        let mut segments = Vec::new();
        let mut literal = String::new();
        let mut rest = raw;
        while let Some(dollar) = rest.find("${") {
            let (before, from_dollar) = rest.split_at(dollar);
            literal.push_str(before);
            let placeholder = [Placeholder::PluginRoot, Placeholder::PluginData]
                .into_iter()
                .find(|p| from_dollar.starts_with(p.spelling()));
            match placeholder {
                Some(p) => {
                    if !literal.is_empty() {
                        segments.push(Segment::Literal(std::mem::take(&mut literal)));
                    }
                    segments.push(Segment::Anchor(p));
                    rest = &from_dollar[p.spelling().len()..];
                }
                None => {
                    // Unrecognized placeholder-like text stays literal.
                    literal.push_str("${");
                    rest = &from_dollar[2..];
                }
            }
        }
        literal.push_str(rest);
        if !literal.is_empty() || segments.is_empty() {
            segments.push(Segment::Literal(literal));
        }
        Self { raw: raw.to_owned(), segments }
    }

    pub fn as_raw(&self) -> &str {
        &self.raw
    }

    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    pub fn has_anchors(&self) -> bool {
        self.segments.iter().any(|s| matches!(s, Segment::Anchor(_)))
    }

    /// Because replacement text lives in its own segment, it is never
    /// rescanned.
    pub fn expand(&self, anchors: &Anchors) -> String {
        let mut out = String::with_capacity(self.raw.len());
        for segment in &self.segments {
            match segment {
                Segment::Literal(text) => out.push_str(text),
                Segment::Anchor(p) => out.push_str(&anchors.path_of(*p).to_string_lossy()),
            }
        }
        out
    }
}

impl fmt::Display for Template {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anchors() -> Anchors {
        Anchors::new("/plugins/devtools", "/plugins/data/devtools")
    }

    #[test]
    fn expands_both_placeholders() {
        let t = Template::parse("${PLUGIN_ROOT}/config:${PLUGIN_DATA}/cache");
        assert_eq!(t.expand(&anchors()), "/plugins/devtools/config:/plugins/data/devtools/cache");
    }

    #[test]
    fn unrecognized_placeholders_stay_literal() {
        let t = Template::parse("${HOME}/x ${PLUGIN_ROOT} $PLUGIN_DATA ${plugin_root}");
        assert_eq!(t.expand(&anchors()), "${HOME}/x /plugins/devtools $PLUGIN_DATA ${plugin_root}");
    }

    #[test]
    fn expansion_is_not_recursive() {
        // An anchor path that itself contains placeholder text must not be
        // expanded again: replacement text is never rescanned.
        let sneaky = Anchors::new("/evil/${PLUGIN_DATA}", "/data");
        let t = Template::parse("${PLUGIN_ROOT}/bin");
        assert_eq!(t.expand(&sneaky), "/evil/${PLUGIN_DATA}/bin");
    }
}
