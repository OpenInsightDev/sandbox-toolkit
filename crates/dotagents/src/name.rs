//! `PluginName` follows *parse, don't validate*: the only way to obtain one is
//! through a parse that enforces every constraint, so any function receiving a
//! `PluginName` may treat validity as settled.

use std::fmt;

/// A validated plugin name: 1–64 characters of `a-z`, `0-9`, `-`, `.`;
/// alphanumeric at both ends; no `--` or `..` runs.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PluginName(String);

/// Exactly which constraint a candidate name broke.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum InvalidName {
    Empty,
    TooLong {
        /// The offending length, in characters.
        length: usize,
    },
    ForbiddenCharacter {
        character: char,
        /// Byte offset in the candidate.
        at: usize,
    },
    EdgeNotAlphanumeric,
    DoubledSeparator {
        /// The repeated separator: `-` or `.`.
        separator: char,
    },
}

impl fmt::Display for InvalidName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("plugin name is empty"),
            Self::TooLong { length } => {
                write!(f, "plugin name is {length} characters long; the limit is 64")
            }
            Self::ForbiddenCharacter { character, at } => write!(
                f,
                "character {character:?} at position {at} is outside `a-z`, `0-9`, `-`, `.`"
            ),
            Self::EdgeNotAlphanumeric => {
                f.write_str("plugin name must start and end with a lowercase letter or digit")
            }
            Self::DoubledSeparator { separator } => {
                write!(f, "plugin name must not repeat {separator:?}")
            }
        }
    }
}

impl std::error::Error for InvalidName {}

impl PluginName {
    pub fn parse(candidate: &str) -> Result<Self, InvalidName> {
        if candidate.is_empty() {
            return Err(InvalidName::Empty);
        }
        let length = candidate.chars().count();
        if length > 64 {
            return Err(InvalidName::TooLong { length });
        }
        for (at, character) in candidate.char_indices() {
            if !matches!(character, 'a'..='z' | '0'..='9' | '-' | '.') {
                return Err(InvalidName::ForbiddenCharacter { character, at });
            }
        }
        let edges_ok = candidate
            .chars()
            .next()
            .zip(candidate.chars().next_back())
            .is_some_and(|(first, last)| {
                first.is_ascii_alphanumeric() && last.is_ascii_alphanumeric()
            });
        if !edges_ok {
            return Err(InvalidName::EdgeNotAlphanumeric);
        }
        if candidate.contains("--") {
            return Err(InvalidName::DoubledSeparator { separator: '-' });
        }
        if candidate.contains("..") {
            return Err(InvalidName::DoubledSeparator { separator: '.' });
        }
        Ok(Self(candidate.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PluginName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for PluginName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::str::FromStr for PluginName {
    type Err = InvalidName;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl TryFrom<String> for PluginName {
    type Error = InvalidName;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl PartialEq<str> for PluginName {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_examples() {
        for ok in ["my-plugin", "acme.tools", "lint3r", "a"] {
            assert!(PluginName::parse(ok).is_ok(), "{ok} should be valid");
        }
        assert_eq!(
            PluginName::parse("My-Plugin"),
            Err(InvalidName::ForbiddenCharacter { character: 'M', at: 0 })
        );
        assert_eq!(PluginName::parse("-start"), Err(InvalidName::EdgeNotAlphanumeric));
        assert_eq!(
            PluginName::parse("has--double"),
            Err(InvalidName::DoubledSeparator { separator: '-' })
        );
        assert_eq!(
            PluginName::parse("too.many..dots"),
            Err(InvalidName::DoubledSeparator { separator: '.' })
        );
        assert_eq!(PluginName::parse(""), Err(InvalidName::Empty));
    }

    #[test]
    fn length_limits() {
        assert!(PluginName::parse(&"a".repeat(64)).is_ok());
        assert_eq!(PluginName::parse(&"a".repeat(65)), Err(InvalidName::TooLong { length: 65 }));
    }
}
