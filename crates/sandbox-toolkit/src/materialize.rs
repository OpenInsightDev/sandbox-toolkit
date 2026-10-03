use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::embed;

/// The directory the tools are released into, under the system cache.
const SUBDIR: &str = "sandbox-toolkit/bin";

/// Releases every embedded tool into the system cache and returns the directory
/// they are in, once all of them are on disk. A tool already holding exactly the
/// embedded bytes is left as it is, so a restart touches no file.
pub async fn materialize() -> io::Result<PathBuf> {
    let dir = dir()?;
    tokio::fs::create_dir_all(&dir).await?;

    let mut tasks = tokio::task::JoinSet::new();
    for tool in &embed::TOOLS {
        let dir = dir.to_owned();
        tasks.spawn_blocking(move || {
            release(&dir, tool).map_err(|error| io::Error::other(format!("{}: {error}", tool.name)))
        });
    }

    // Every task is drained so that no write outlives startup; the first failure
    // is the one reported.
    let mut failure = None;
    while let Some(joined) = tasks.join_next().await {
        let result = match joined {
            Ok(result) => result,
            Err(error) => Err(io::Error::other(error)),
        };
        if let Err(error) = result {
            failure.get_or_insert(error);
        }
    }

    match failure {
        Some(error) => Err(error),
        None => Ok(dir),
    }
}

fn dir() -> io::Result<PathBuf> {
    dirs::cache_dir()
        .map(|cache| cache.join(SUBDIR))
        .ok_or_else(|| io::Error::other("the system cache directory is unavailable"))
}

fn release(dir: &Path, tool: &embed::Tool) -> io::Result<()> {
    let path = dir.join(tool.name);

    if digest(&path) == Some(*tool.digest) {
        return Ok(());
    }

    fs::write(&path, tool.bytes()?)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
}

/// The digest of the file at `path`, `None` when there is no readable file
/// there to compare against the embedded one.
fn digest(path: &Path) -> Option<[u8; 32]> {
    let mut file = fs::File::open(path).ok()?;
    let mut hasher = blake3::Hasher::new();
    io::copy(&mut file, &mut hasher).ok()?;

    Some(*hasher.finalize().as_bytes())
}
