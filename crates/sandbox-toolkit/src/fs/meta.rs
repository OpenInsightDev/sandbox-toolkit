use std::time::{SystemTime, UNIX_EPOCH};

/// A system time as RFC 3339, the encoding the resource model uses for all of
/// its timestamps.
pub(crate) fn timestamp(time: SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(time).to_rfc3339()
}

/// A strong validator over the file facts the server can cheaply observe.
pub(crate) fn etag(metadata: &std::fs::Metadata) -> String {
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();

    format!("\"{}-{}\"", metadata.len(), modified)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::registry::test_support::TempDir;

    #[test]
    fn quotes_the_length_and_the_modification_time() {
        let dir = TempDir::new();
        std::fs::write(dir.path().join("note.txt"), "hello").unwrap();
        let metadata = std::fs::metadata(dir.path().join("note.txt")).unwrap();

        let tag = etag(&metadata);

        assert!(tag.starts_with("\"5-"));
        assert!(tag.ends_with('"'));
    }
}
