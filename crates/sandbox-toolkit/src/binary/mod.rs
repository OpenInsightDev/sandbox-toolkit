//! Bundled into the server, so a release needs no network to materialize them.
//! `build.rs` supplies the payloads as gzipped bytes in `OUT_DIR`.

use std::{
    io::{self, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use flate2::read::GzDecoder;
use thiserror::Error;

const BINARIES_DIR: &str = "sandbox-toolkit-binaries";

#[derive(Debug, Error)]
pub(crate) enum BinaryError {
    #[error("failed to create binaries directory {}", dir.display())]
    CreateDir {
        dir: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to materialize binary `{binary}` to {}", destination.display())]
    Materialize {
        binary: &'static str,
        destination: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("binary materialization task failed")]
    Task(#[from] tokio::task::JoinError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Payload {
    /// Embedded verbatim, for a binary that has no release archive to fetch.
    #[allow(dead_code)]
    Raw(&'static [u8]),
    Gzip(&'static [u8]),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Binary {
    pub(crate) name: &'static str,
    pub(crate) payload: Payload,
}

/// `fd` — a simple, fast and user-friendly alternative to `find`.
pub(crate) const FD: Binary = Binary {
    name: "fd",
    payload: Payload::Gzip(include_bytes!(concat!(env!("OUT_DIR"), "/fd.gz"))),
};

/// `rg` — ripgrep, a line-oriented recursive search tool that respects
/// `.gitignore` by default.
pub(crate) const RIPGREP: Binary = Binary {
    name: "rg",
    payload: Payload::Gzip(include_bytes!(concat!(env!("OUT_DIR"), "/rg.gz"))),
};

/// `jaq` — "just another JSON query tool", a `jq` clone.
#[cfg(feature = "jaq")]
pub(crate) const JAQ: Binary = Binary {
    name: "jaq",
    payload: Payload::Gzip(include_bytes!(concat!(env!("OUT_DIR"), "/jaq.gz"))),
};

/// `uv` — an extremely fast Python package and project manager.
#[cfg(feature = "uv")]
pub(crate) const UV: Binary = Binary {
    name: "uv",
    payload: Payload::Gzip(include_bytes!(concat!(env!("OUT_DIR"), "/uv.gz"))),
};

/// `uvx` — run a Python tool without installing it, `uv`'s `pipx` equivalent.
///
/// It execs the `uv` sitting next to it, so both must land in the same directory.
#[cfg(feature = "uv")]
pub(crate) const UVX: Binary = Binary {
    name: "uvx",
    payload: Payload::Gzip(include_bytes!(concat!(env!("OUT_DIR"), "/uvx.gz"))),
};

/// `deno` — a secure JavaScript and TypeScript runtime.
#[cfg(feature = "deno")]
pub(crate) const DENO: Binary = Binary {
    name: "deno",
    payload: Payload::Gzip(include_bytes!(concat!(env!("OUT_DIR"), "/deno.gz"))),
};

pub(crate) fn bundled() -> Vec<Binary> {
    #[allow(unused_mut)]
    let mut binaries = vec![FD, RIPGREP];

    #[cfg(feature = "jaq")]
    binaries.push(JAQ);

    #[cfg(feature = "uv")]
    {
        binaries.push(UV);
        binaries.push(UVX);
    }

    #[cfg(feature = "deno")]
    binaries.push(DENO);

    binaries
}

/// The stable directory the bundled executables are materialized into.
///
/// The path is derived deterministically from the system temporary directory
/// rather than from a random `tempfile::TempDir`, so it stays the same across
/// restarts and can be referenced by the sandboxes the server spawns later.
pub(crate) fn materialized_dir() -> PathBuf {
    std::env::temp_dir().join(BINARIES_DIR)
}

/// Each executable is written to a temporary name and atomically renamed into
/// place, so a concurrent reader never sees a partial binary.
pub(crate) async fn materialize() -> Result<PathBuf, BinaryError> {
    let dir = materialized_dir();
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|source| BinaryError::CreateDir {
            dir: dir.clone(),
            source,
        })?;

    let mut tasks = tokio::task::JoinSet::new();
    for binary in bundled() {
        let dir = dir.clone();
        tasks.spawn_blocking(move || materialize_one(binary, dir));
    }

    while let Some(joined) = tasks.join_next().await {
        // A panicked task must not be silently ignored.
        joined??;
    }

    Ok(dir)
}

fn materialize_one(binary: Binary, dir: PathBuf) -> Result<(), BinaryError> {
    let destination = dir.join(binary.name);
    let staging = dir.join(format!(
        ".{}.{}.{}.tmp",
        binary.name,
        std::process::id(),
        STAGING_COUNTER.fetch_add(1, Ordering::Relaxed),
    ));

    write_payload(binary, &staging, &destination)?;

    std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o755))
        .map_err(|source| materialize_error(binary.name, &destination, source))?;

    std::fs::rename(&staging, &destination)
        .map_err(|source| materialize_error(binary.name, &destination, source))?;

    Ok(())
}

static STAGING_COUNTER: AtomicU64 = AtomicU64::new(0);

fn write_payload(binary: Binary, staging: &Path, destination: &Path) -> Result<(), BinaryError> {
    let mut file = std::fs::File::create(staging)
        .map_err(|source| materialize_error(binary.name, destination, source))?;

    match binary.payload {
        Payload::Raw(bytes) => file
            .write_all(bytes)
            .map_err(|source| materialize_error(binary.name, destination, source))?,
        Payload::Gzip(bytes) => {
            let mut decoder = GzDecoder::new(bytes);
            io::copy(&mut decoder, &mut file)
                .map_err(|source| materialize_error(binary.name, destination, source))?;
        }
    }

    file.flush()
        .map_err(|source| materialize_error(binary.name, destination, source))?;

    Ok(())
}

fn materialize_error(binary: &'static str, destination: &Path, source: io::Error) -> BinaryError {
    BinaryError::Materialize {
        binary,
        destination: destination.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::*;

    fn payload_bytes(binary: Binary) -> Vec<u8> {
        match binary.payload {
            Payload::Raw(bytes) => bytes.to_vec(),
            Payload::Gzip(bytes) => {
                let mut out = Vec::new();
                GzDecoder::new(bytes)
                    .read_to_end(&mut out)
                    .expect("inflating the payload failed");
                out
            }
        }
    }

    #[test]
    fn every_binary_carries_a_payload() {
        for binary in bundled() {
            assert!(
                !payload_bytes(binary).is_empty(),
                "{} was embedded empty",
                binary.name
            );
        }
    }

    #[tokio::test]
    async fn materialize_writes_every_binary() {
        let dir = materialize().await.expect("materializing binaries failed");

        assert!(dir.is_absolute(), "{} is not absolute", dir.display());
        assert_eq!(dir, materialized_dir());

        for binary in bundled() {
            let path = dir.join(binary.name);
            assert!(path.is_file(), "{} was not materialized", path.display());
            assert_eq!(
                std::fs::read(&path).unwrap(),
                payload_bytes(binary),
                "{} does not match its embedded payload",
                binary.name
            );
        }
    }

    #[test]
    fn materialized_dir_is_stable() {
        assert_eq!(materialized_dir(), materialized_dir());
    }

    #[tokio::test]
    async fn embedded_binaries_are_runnable() {
        let dir = materialize().await.expect("materializing binaries failed");

        for binary in bundled() {
            let output = std::process::Command::new(dir.join(binary.name))
                .arg("--version")
                .output()
                .unwrap_or_else(|error| panic!("failed to run {}: {error}", binary.name));

            assert!(
                output.status.success(),
                "{} --version exited with {}: {}",
                binary.name,
                output.status,
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}
