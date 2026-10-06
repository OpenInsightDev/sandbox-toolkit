use std::io;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

/// The uploads tusd stages in its upload directory, addressed by the id a `/tus`
/// upload URL ends in.
#[derive(Debug, Clone)]
pub struct Uploads {
    dir: PathBuf,
}

#[derive(Debug, Error)]
pub enum CommitError {
    #[error("upload `{0}` was not found")]
    NotFound(String),
    #[error("upload `{0}` has not finished")]
    Incomplete(String),
    #[error(transparent)]
    Io(#[from] io::Error),
}

impl Uploads {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// Moves a finished upload onto `destination` and spends it: its info file
    /// goes, so tusd reports the id as gone and a second `commit` cannot reuse
    /// it. The move is a rename, so it fails across devices exactly as `move`
    /// does, leaving the staged upload to be retried.
    pub async fn commit(&self, id: &str, destination: &Path) -> Result<(), CommitError> {
        let (data, info_path) = self.paths(id)?;

        let raw = tokio::fs::read(&info_path)
            .await
            .map_err(|error| missing(id, error))?;
        let info: FileInfo = serde_json::from_slice(&raw)
            .map_err(|error| CommitError::Io(io::Error::other(error)))?;
        let staged = tokio::fs::metadata(&data)
            .await
            .map_err(|error| missing(id, error))?
            .len();
        if !info.finished(staged) {
            return Err(CommitError::Incomplete(id.to_owned()));
        }

        tokio::fs::rename(&data, destination).await?;
        tokio::fs::remove_file(&info_path).await?;

        Ok(())
    }

    /// The data and info files of `id`. An id that is not one path component
    /// cannot name an upload and is rejected before it reaches the disk.
    fn paths(&self, id: &str) -> Result<(PathBuf, PathBuf), CommitError> {
        let mut parts = Path::new(id).components();
        if !matches!(
            (parts.next(), parts.next()),
            (Some(Component::Normal(_)), None)
        ) {
            return Err(CommitError::NotFound(id.to_owned()));
        }

        Ok((self.dir.join(id), self.dir.join(format!("{id}.info"))))
    }
}

/// The subset of tusd's `.info` JSON that says whether an upload finished: its
/// byte count against the size declared when it was created.
#[derive(Deserialize)]
struct FileInfo {
    #[serde(rename = "Size", default)]
    size: u64,
    #[serde(rename = "SizeIsDeferred", default)]
    size_is_deferred: bool,
}

impl FileInfo {
    fn finished(&self, staged: u64) -> bool {
        !self.size_is_deferred && staged == self.size
    }
}

fn missing(id: &str, error: io::Error) -> CommitError {
    if error.kind() == io::ErrorKind::NotFound {
        CommitError::NotFound(id.to_owned())
    } else {
        CommitError::Io(error)
    }
}
