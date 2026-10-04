use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use thiserror::Error;
use tokio::task::{JoinError, JoinSet};

use super::embed;

/// The directory the tools are released into, under the system cache.
const SUBDIR: &str = "sandbox-toolkit/bin";

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("failed to release `{name}`: {source}")]
    Release {
        name: &'static str,
        #[source]
        source: io::Error,
    },
    #[error(transparent)]
    Task(#[from] JoinError),
}

/// Releases every embedded tool into the system cache and returns the directory
/// they are in, once all of them are on disk. A tool already holding exactly the
/// embedded bytes is left as it is, so a restart touches no file.
pub async fn materialize() -> Result<PathBuf, Error> {
    let dir = dir()?;
    tokio::fs::create_dir_all(&dir).await?;

    let mut tasks = JoinSet::new();
    for tool in &embed::TOOLS {
        let dir = dir.clone();
        tasks.spawn_blocking(move || release(&dir, tool));
    }

    // `join_all` panics on a failed task, so the set is joined by hand to report
    // the failure instead. Tasks still running when this returns are awaited by
    // runtime shutdown, which waits for every `spawn_blocking` it started.
    while let Some(released) = tasks.join_next().await {
        released??;
    }

    Ok(dir)
}

fn dir() -> io::Result<PathBuf> {
    dirs::cache_dir()
        .map(|cache| cache.join(SUBDIR))
        .ok_or_else(|| io::Error::other("the system cache directory is unavailable"))
}

fn release(dir: &Path, tool: &embed::Tool) -> Result<(), Error> {
    let path = dir.join(tool.name);

    if digest(&path) == Some(*tool.digest) {
        return Ok(());
    }

    let name = tool.name;
    let named = |source| Error::Release { name, source };

    let bytes = tool.bytes().map_err(named)?;
    fs::write(&path, bytes).map_err(named)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).map_err(named)
}

/// The digest of the file at `path`, `None` when there is no readable file
/// there to compare against the embedded one.
fn digest(path: &Path) -> Option<[u8; 32]> {
    let mut file = fs::File::open(path).ok()?;
    let mut hasher = blake3::Hasher::new();
    io::copy(&mut file, &mut hasher).ok()?;

    Some(*hasher.finalize().as_bytes())
}
