use std::fmt;

/// A client selects validation rules from the `$schema` a document declares,
/// never by fetching them, so this enum is that selection: closed, local,
/// offline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SpecVersion {
    V1_0_0,
}

impl fmt::Display for SpecVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("1.0.0")
    }
}
