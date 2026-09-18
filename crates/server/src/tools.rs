//! Tools bundled into the server binary, so a release needs no network to
//! materialize them. `build.rs` supplies the payloads; `tgrep` comes from
//! `bindeps`.

use std::{
    ffi::{OsStr, OsString},
    io::{self, Write},
    os::unix::fs::PermissionsExt,
    path::Path,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use flate2::read::GzDecoder;
use thiserror::Error;

/// Name of the directory, inside the system temporary directory, that the
/// bundled executables are materialized into.
#[allow(dead_code)]
const TOOLS_DIR: &str = "sandbox-toolkit-tools";

/// Why materializing the bundled executables failed.
#[derive(Debug, Error)]
pub enum ToolError {
    /// The tools directory could not be created.
    #[error("failed to create tools directory {}", dir.display())]
    CreateDir {
        /// Directory that could not be created.
        dir: PathBuf,
        #[source]
        source: io::Error,
    },
    /// A bundled executable could not be written into place.
    #[error("failed to materialize tool `{tool}` to {}", destination.display())]
    Materialize {
        /// Name of the tool that failed to materialize.
        tool: &'static str,
        /// Path the tool was being written to.
        destination: PathBuf,
        #[source]
        source: io::Error,
    },
    /// A materialization task panicked instead of returning.
    #[error("tool materialization task failed")]
    Task(#[from] tokio::task::JoinError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Payload {
    #[allow(dead_code)]
    Raw(&'static [u8]),
    Gzip(&'static [u8]),
}

/// A single bundled executable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tool {
    /// Name the executable is invoked as.
    pub name: &'static str,
    pub payload: Payload,
}

/// `fd` — a simple, fast and user-friendly alternative to `find`.
pub const FD: Tool = Tool {
    name: "fd",
    payload: Payload::Gzip(include_bytes!(concat!(env!("OUT_DIR"), "/fd.gz"))),
};

/// `rg` — ripgrep, a line-oriented recursive search tool that respects
/// `.gitignore` by default.
pub const RIPGREP: Tool = Tool {
    name: "rg",
    payload: Payload::Gzip(include_bytes!(concat!(env!("OUT_DIR"), "/rg.gz"))),
};

/// `jaq` — "just another JSON query tool", a `jq` clone.
#[cfg(feature = "jaq")]
pub const JAQ: Tool = Tool {
    name: "jaq",
    payload: Payload::Gzip(include_bytes!(concat!(env!("OUT_DIR"), "/jaq.gz"))),
};

/// `tgrep` — a toy grep that respects `.gitignore`.
#[cfg(feature = "tgrep")]
pub const TGREP: Tool = Tool {
    name: "tgrep",
    payload: Payload::Raw(include_bytes!(env!("CARGO_BIN_FILE_TGREP_tgrep"))),
};

/// `uv` — an extremely fast Python package and project manager.
#[cfg(feature = "uv")]
pub const UV: Tool = Tool {
    name: "uv",
    payload: Payload::Gzip(include_bytes!(concat!(env!("OUT_DIR"), "/uv.gz"))),
};

/// `uvx` — run a Python tool without installing it, `uv`'s `pipx` equivalent.
///
/// The binary execs the `uv` sitting next to it, so it only works once both
/// have been materialized into the same directory.
#[cfg(feature = "uv")]
pub const UVX: Tool = Tool {
    name: "uvx",
    payload: Payload::Gzip(include_bytes!(concat!(env!("OUT_DIR"), "/uvx.gz"))),
};

/// `deno` — a secure JavaScript and TypeScript runtime.
#[cfg(feature = "deno")]
pub const DENO: Tool = Tool {
    name: "deno",
    payload: Payload::Gzip(include_bytes!(concat!(env!("OUT_DIR"), "/deno.gz"))),
};

/// Every executable embedded in this build.
pub fn bundled() -> Vec<Tool> {
    #[allow(unused_mut)]
    let mut tools = vec![FD, RIPGREP];

    #[cfg(feature = "tgrep")]
    tools.push(TGREP);

    #[cfg(feature = "jaq")]
    tools.push(JAQ);

    #[cfg(feature = "uv")]
    {
        tools.push(UV);
        tools.push(UVX);
    }

    #[cfg(feature = "deno")]
    tools.push(DENO);

    tools
}

/// Look up a bundled executable by the name it is invoked as.
pub fn find(name: &str) -> Option<Tool> {
    bundled().into_iter().find(|tool| tool.name == name)
}

/// The stable directory the bundled executables are materialized into.
///
/// The path is derived deterministically from the system temporary directory
/// rather than from a random `tempfile::TempDir`, so it stays the same across
/// restarts and can be referenced by the sandboxes the server spawns later.
pub fn materialized_dir() -> PathBuf {
    std::env::temp_dir().join(TOOLS_DIR)
}

/// The `PATH` a spawned command should see: [`materialized_dir`] first, then
/// `inherited`, so the bundled executables resolve by name alongside whatever
/// the caller already had on `PATH`.
pub fn search_path(inherited: Option<&OsStr>) -> OsString {
    let tools = materialized_dir();
    match inherited {
        Some(existing) => {
            std::env::join_paths(std::iter::once(tools).chain(std::env::split_paths(existing)))
                .unwrap_or_else(|_| existing.to_os_string())
        }
        None => tools.into_os_string(),
    }
}

/// Inflate every embedded executable into [`materialized_dir`], each written to
/// a temporary name and atomically renamed into place. Returns that directory.
pub async fn materialize() -> Result<PathBuf, ToolError> {
    let dir = materialized_dir();
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|source| ToolError::CreateDir {
            dir: dir.clone(),
            source,
        })?;

    let mut tasks = tokio::task::JoinSet::new();
    for tool in bundled() {
        let dir = dir.clone();
        tasks.spawn_blocking(move || materialize_one(tool, dir));
    }

    while let Some(joined) = tasks.join_next().await {
        // A panicked task must not be silently ignored.
        joined??;
    }

    Ok(dir)
}

/// Write a single tool into `dir`, replacing any previous copy atomically.
fn materialize_one(tool: Tool, dir: PathBuf) -> Result<(), ToolError> {
    let destination = dir.join(tool.name);
    let staging = dir.join(format!(
        ".{}.{}.{}.tmp",
        tool.name,
        std::process::id(),
        STAGING_COUNTER.fetch_add(1, Ordering::Relaxed),
    ));

    write_payload(tool, &staging, &destination)?;

    std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o755))
        .map_err(|source| materialize_error(tool.name, &destination, source))?;

    std::fs::rename(&staging, &destination)
        .map_err(|source| materialize_error(tool.name, &destination, source))?;

    Ok(())
}

static STAGING_COUNTER: AtomicU64 = AtomicU64::new(0);

fn write_payload(tool: Tool, staging: &Path, destination: &Path) -> Result<(), ToolError> {
    let mut file = std::fs::File::create(staging)
        .map_err(|source| materialize_error(tool.name, destination, source))?;

    match tool.payload {
        Payload::Raw(bytes) => file
            .write_all(bytes)
            .map_err(|source| materialize_error(tool.name, destination, source))?,
        Payload::Gzip(bytes) => {
            let mut decoder = GzDecoder::new(bytes);
            io::copy(&mut decoder, &mut file)
                .map_err(|source| materialize_error(tool.name, destination, source))?;
        }
    }

    file.flush()
        .map_err(|source| materialize_error(tool.name, destination, source))?;

    Ok(())
}

fn materialize_error(tool: &'static str, destination: &Path, source: io::Error) -> ToolError {
    ToolError::Materialize {
        tool,
        destination: destination.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::*;

    fn payload_bytes(tool: Tool) -> Vec<u8> {
        match tool.payload {
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
    fn every_tool_carries_a_payload() {
        for tool in bundled() {
            assert!(
                !payload_bytes(tool).is_empty(),
                "{} was embedded empty",
                tool.name
            );
        }
    }

    #[tokio::test]
    async fn materialize_writes_every_tool() {
        let dir = materialize().await.expect("materializing tools failed");

        assert!(dir.is_absolute(), "{} is not absolute", dir.display());
        assert_eq!(dir, materialized_dir());

        for tool in bundled() {
            let path = dir.join(tool.name);
            assert!(path.is_file(), "{} was not materialized", path.display());
            assert_eq!(
                std::fs::read(&path).unwrap(),
                payload_bytes(tool),
                "{} does not match its embedded payload",
                tool.name
            );
        }
    }

    #[test]
    fn materialized_dir_is_stable() {
        assert_eq!(materialized_dir(), materialized_dir());
    }

    #[tokio::test]
    async fn embedded_binaries_are_runnable() {
        let dir = materialize().await.expect("materializing tools failed");

        #[allow(unused_mut)]
        let mut tools = vec![FD, RIPGREP];
        #[cfg(feature = "jaq")]
        tools.push(JAQ);
        #[cfg(feature = "uv")]
        {
            tools.push(UV);
            tools.push(UVX);
        }
        #[cfg(feature = "deno")]
        tools.push(DENO);

        for tool in tools {
            let output = std::process::Command::new(dir.join(tool.name))
                .arg("--version")
                .output()
                .unwrap_or_else(|error| panic!("failed to run {}: {error}", tool.name));

            assert!(
                output.status.success(),
                "{} --version exited with {}: {}",
                tool.name,
                output.status,
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}
