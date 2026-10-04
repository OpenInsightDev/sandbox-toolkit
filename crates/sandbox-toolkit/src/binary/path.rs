use std::ffi::OsString;
use std::path::Path;

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
