//! Paths *inside* a package.
//!
//! The specification talks about two kinds of paths, and each gets its own
//! type so the distinction cannot be lost in a `String`:
//!
//! * [`PackagePath`] — where a file lives inside the package
//!   (`plugin.json`, `skills/deploy/SKILL.md`). Always relative to the package
//!   root, always `/`-separated, never escaping. Only this crate constructs
//!   them, from the fixed component locations and from directory listings.
//! * [`RelativePath`] — a *configured* package-relative path: user input that
//!   must begin with `./` and must stay inside the package root. Parsing one
//!   is fallible; holding one is proof it passed.

use std::fmt;

/// A `/`-separated path to a file or directory inside a package, relative to
/// the package root.
///
/// Constructed only by the loader — from the fixed component locations and
/// from names returned by directory listings — so a `PackagePath` is always
/// well-formed and root-relative.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PackagePath(String);

impl PackagePath {
    pub(crate) fn new(path: impl Into<String>) -> Self {
        Self(path.into())
    }

    pub(crate) fn child(&self, name: &str) -> Self {
        Self(format!("{}/{}", self.0, name))
    }

    /// The path as its canonical `/`-separated string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Join onto a native root directory, converting `/` to the platform
    /// separator component by component.
    pub fn to_native(&self, root: &std::path::Path) -> std::path::PathBuf {
        let mut out = root.to_path_buf();
        for segment in self.0.split('/') {
            out.push(segment);
        }
        out
    }
}

impl fmt::Display for PackagePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for PackagePath {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// A configured package-relative path: begins with `./` and, resolved purely
/// lexically, never leaves the package root.
///
/// This is the type for values a package *author* wrote — an MCP `command` of
/// `./bin/server`, a `cwd` of `./data`. Successful parsing is the containment
/// proof; APIs that need a safe path ask for a `RelativePath`, not a string.
///
/// Lexical containment is necessary but not sufficient: symlinks can still
/// escape at the filesystem level, which is why loading also checks whether
/// the *resolved* path stays confined.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RelativePath(String);

/// Why a string was refused as a package-relative path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelativePathError {
    /// The path does not begin with `./`.
    MissingDotSlash,
    /// Traversal (`..`) would resolve outside the package root.
    EscapesRoot,
}

impl fmt::Display for RelativePathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingDotSlash => f.write_str("package-relative paths must begin with `./`"),
            Self::EscapesRoot => f.write_str("path resolves outside the package root"),
        }
    }
}

impl std::error::Error for RelativePathError {}

impl RelativePath {
    /// Parse a configured value as a package-relative path.
    pub fn parse(raw: &str) -> Result<Self, RelativePathError> {
        let Some(rest) = raw.strip_prefix("./") else {
            return Err(RelativePathError::MissingDotSlash);
        };
        if !lexically_contained(rest) {
            return Err(RelativePathError::EscapesRoot);
        }
        Ok(Self(raw.to_owned()))
    }

    /// The path exactly as written, including the leading `./`.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Resolve against a native package root.
    pub fn resolve(&self, root: &std::path::Path) -> std::path::PathBuf {
        let mut out = root.to_path_buf();
        for segment in self.0[2..].split('/').filter(|s| !s.is_empty() && *s != ".") {
            if segment == ".." {
                out.pop();
            } else {
                out.push(segment);
            }
        }
        out
    }
}

impl fmt::Display for RelativePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::str::FromStr for RelativePath {
    type Err = RelativePathError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

/// Would this `/`-separated path, resolved lexically, stay at or below its
/// starting directory? (`a/../b` yes, `..` no, `a/../../b` no.)
pub(crate) fn lexically_contained(path: &str) -> bool {
    let mut depth: i32 = 0;
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            _ => depth += 1,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_require_dot_slash() {
        assert_eq!(RelativePath::parse("bin/server"), Err(RelativePathError::MissingDotSlash));
        assert!(RelativePath::parse("./bin/server").is_ok());
    }

    #[test]
    fn relative_paths_must_stay_inside() {
        assert!(RelativePath::parse("./a/../b").is_ok());
        assert_eq!(RelativePath::parse("./a/../../b"), Err(RelativePathError::EscapesRoot));
        assert_eq!(RelativePath::parse("./.."), Err(RelativePathError::EscapesRoot));
    }

    #[test]
    fn resolve_joins_lexically() {
        let p = RelativePath::parse("./data/./cache").unwrap();
        assert_eq!(
            p.resolve(std::path::Path::new("/root")),
            std::path::PathBuf::from("/root/data/cache")
        );
    }
}
