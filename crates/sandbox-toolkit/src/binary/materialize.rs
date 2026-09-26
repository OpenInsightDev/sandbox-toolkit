use std::{
    env,
    ffi::OsString,
    fs,
    io::{self, BufReader, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use super::{
    BinaryError,
    catalog::{self, Binary},
};

/// Only the owner may read or traverse the directory.
const DIR_MODE: u32 = 0o700;

/// Every materialized file is executable.
const FILE_MODE: u32 = 0o755;

/// The directory the bundled executables are materialized into.
///
/// The path is derived from `$HOME` alone, so it stays the same across restarts
/// and can be referenced by the sandboxes the server spawns later.
pub(crate) fn materialized_dir() -> Result<PathBuf, BinaryError> {
    dir_under(env::var_os("HOME"))
}

pub(super) fn dir_under(home: Option<OsString>) -> Result<PathBuf, BinaryError> {
    let home = home
        .filter(|home| !home.is_empty())
        .ok_or(BinaryError::MissingHome)?;
    Ok(PathBuf::from(home).join(".sandbox-toolkit").join("bin"))
}

/// Materialize every bundled tool once, in parallel, before the server starts.
///
/// Existing files are left untouched when they already hash to their embedded
/// digest. Failure of any single tool fails the whole call; already materialized
/// tools are not rolled back, so the next start resumes from their hashes.
pub(crate) async fn materialize() -> Result<PathBuf, BinaryError> {
    let dir = materialized_dir()?;
    materialize_into(&dir).await?;
    Ok(dir)
}

/// The directory creation and parallel expansion shared by [`materialize`] and the
/// tests, which point it at a scratch directory instead of `$HOME`.
pub(super) async fn materialize_into(dir: &Path) -> Result<(), BinaryError> {
    tokio::fs::DirBuilder::new()
        .recursive(true)
        .mode(DIR_MODE)
        .create(dir)
        .await
        .map_err(|source| BinaryError::CreateDir {
            dir: dir.to_path_buf(),
            source,
        })?;

    let mut tasks = tokio::task::JoinSet::new();
    for binary in catalog::bundled() {
        let dir = dir.to_path_buf();
        tasks.spawn_blocking(move || materialize_one(binary, &dir));
    }

    while let Some(joined) = tasks.join_next().await {
        // A panicked task must not be silently ignored.
        joined??;
    }

    Ok(())
}

/// Each executable is written to a temporary name in the same directory and
/// atomically renamed into place, so a concurrent reader never sees a partial
/// binary and concurrent materializations cannot corrupt one another.
fn materialize_one(binary: Binary, dir: &Path) -> Result<(), BinaryError> {
    let destination = dir.join(binary.name);
    let digest = binary.digest()?;
    if is_current(&destination, binary.name, &digest)? {
        return Ok(());
    }

    let staging = staging_path(dir, binary.name);
    if let Err(error) = stage(binary, &staging, &destination, &digest) {
        // The staging file is ours and incomplete; never leave it behind.
        let _ = fs::remove_file(&staging);
        return Err(error);
    }

    Ok(())
}

/// Write the payload to the staging file, mark it executable, then publish it with
/// a rename, so the destination is complete and executable the moment it appears.
fn stage(
    binary: Binary,
    staging: &Path,
    destination: &Path,
    digest: &[u8; 32],
) -> Result<(), BinaryError> {
    write_payload(binary, staging, destination, digest)?;
    fs::set_permissions(staging, fs::Permissions::from_mode(FILE_MODE))
        .map_err(|source| materialize_error(binary.name, destination, source))?;
    fs::rename(staging, destination)
        .map_err(|source| materialize_error(binary.name, destination, source))?;
    Ok(())
}

/// A file is current only when it exists and hashes to the embedded digest. A
/// missing file is not an error; any other read failure is.
fn is_current(path: &Path, binary: &'static str, digest: &[u8; 32]) -> Result<bool, BinaryError> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(source) => return Err(read_error(binary, path, source)),
    };

    let mut hasher = blake3::Hasher::new();
    io::copy(&mut BufReader::new(file), &mut hasher)
        .map_err(|source| read_error(binary, path, source))?;

    Ok(hasher.finalize().as_bytes() == digest)
}

/// Decompress the embedded payload straight into the staging file while hashing
/// it, so a corrupted payload is rejected before it can be renamed into place.
fn write_payload(
    binary: Binary,
    staging: &Path,
    destination: &Path,
    expected: &[u8; 32],
) -> Result<(), BinaryError> {
    let mut decoder = zstd::stream::read::Decoder::new(binary.payload)
        .map_err(|source| materialize_error(binary.name, destination, source))?;
    let file = fs::File::create(staging)
        .map_err(|source| materialize_error(binary.name, destination, source))?;
    let mut sink = DigestingWriter::new(file);

    io::copy(&mut decoder, &mut sink)
        .map_err(|source| materialize_error(binary.name, destination, source))?;
    sink.flush()
        .map_err(|source| materialize_error(binary.name, destination, source))?;

    if sink.digest() != *expected {
        return Err(BinaryError::Corrupt {
            binary: binary.name,
        });
    }

    Ok(())
}

/// Forwards writes to the staging file and folds every byte into a blake3 hash.
struct DigestingWriter<W> {
    inner: W,
    hasher: blake3::Hasher,
}

impl<W> DigestingWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            hasher: blake3::Hasher::new(),
        }
    }

    fn digest(&self) -> [u8; 32] {
        *self.hasher.finalize().as_bytes()
    }
}

impl<W: Write> Write for DigestingWriter<W> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(buffer)?;
        self.hasher.update(&buffer[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

static STAGING_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A hidden sibling of the destination, unique per process and call, so concurrent
/// materializations never share a staging file.
fn staging_path(dir: &Path, name: &str) -> PathBuf {
    let sequence = STAGING_COUNTER.fetch_add(1, Ordering::Relaxed);
    dir.join(format!(".{name}.{}.{sequence}.tmp", std::process::id()))
}

fn materialize_error(binary: &'static str, destination: &Path, source: io::Error) -> BinaryError {
    BinaryError::Materialize {
        binary,
        destination: destination.to_path_buf(),
        source,
    }
}

fn read_error(binary: &'static str, path: &Path, source: io::Error) -> BinaryError {
    BinaryError::Read {
        binary,
        path: path.to_path_buf(),
        source,
    }
}
