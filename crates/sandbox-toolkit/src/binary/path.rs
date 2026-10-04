use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

/// The toolkit's cache directory: the materialized tools and the sidecar state
/// live under it.
pub fn cache() -> io::Result<PathBuf> {
    dirs::cache_dir()
        .map(|cache| cache.join(CACHE))
        .ok_or_else(|| io::Error::other("the system cache directory is unavailable"))
}

const CACHE: &str = "sandbox-toolkit";

/// Joined by hand because `join_paths` rejects a directory holding the separator
/// instead of returning a `PATH`.
pub fn for_subprocess(bin: &Path) -> OsString {
    let mut path = bin.as_os_str().to_owned();

    if let Some(host) = std::env::var_os("PATH") {
        path.push(":");
        path.push(host);
    }

    path
}
