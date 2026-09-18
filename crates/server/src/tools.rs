//! Command-line tools the server bundles for the sandboxes it manages.
//!
//! Every entry is a cargo *binary dependency* (RFC 3028): cargo compiles the
//! upstream crate's executable as part of this build and hands us its path at
//! compile time through `CARGO_BIN_FILE_<DEP>_<BIN>`, so nothing has to be
//! installed on the host running the server.
//!
//! Artifacts are produced by cargo itself, not by this crate, and are rebuilt
//! whenever the dependency is updated.

use std::path::PathBuf;

/// Name of the directory, inside the system temporary directory, that the
/// bundled executables are materialized into.
const TOOLS_DIR: &str = "sandbox-toolkit-tools";

/// A single bundled executable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tool {
    /// Name the executable is invoked as.
    pub name: &'static str,
    /// Path to the compiled executable inside cargo's target directory.
    pub path: &'static str,
}

/// `fd` — a simple, fast and user-friendly alternative to `find`.
pub const FD: Tool = Tool {
    name: "fd",
    path: env!("CARGO_BIN_FILE_FD_FIND_fd"),
};

/// `rg` — ripgrep, a line-oriented recursive search tool that respects
/// `.gitignore` by default.
pub const RIPGREP: Tool = Tool {
    name: "rg",
    path: env!("CARGO_BIN_FILE_RIPGREP_rg"),
};

/// `jaq` — "just another JSON query tool", a `jq` clone.
#[cfg(feature = "jaq")]
pub const JAQ: Tool = Tool {
    name: "jaq",
    path: env!("CARGO_BIN_FILE_JAQ_jaq"),
};

/// `tgrep` — a toy grep that respects `.gitignore`.
#[cfg(feature = "tgrep")]
pub const TGREP: Tool = Tool {
    name: "tgrep",
    path: env!("CARGO_BIN_FILE_TGREP_tgrep"),
};

/// Every executable compiled into this build.
///
/// The list grows with the `tgrep` and `jaq` cargo features.
pub fn bundled() -> Vec<Tool> {
    #[allow(unused_mut)]
    let mut tools = vec![FD, RIPGREP];

    #[cfg(feature = "tgrep")]
    tools.push(TGREP);

    #[cfg(feature = "jaq")]
    tools.push(JAQ);

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

/// Copy every bundled executable into [`materialized_dir`].
///
/// The copies run concurrently, one task per tool, and each binary is written
/// to a temporary name and then atomically renamed into place so a reader can
/// never observe a partially written executable.
///
/// Returns the directory the tools were written to.
pub async fn materialize() -> std::io::Result<PathBuf> {
    let dir = materialized_dir();
    tokio::fs::create_dir_all(&dir).await?;

    let mut tasks = tokio::task::JoinSet::new();
    for tool in bundled() {
        tasks.spawn(materialize_one(tool, dir.clone()));
    }

    while let Some(joined) = tasks.join_next().await {
        // A panicked task must not be silently ignored.
        joined.expect("tool materialization task panicked")?;
    }

    Ok(dir)
}

/// Copy a single tool into `dir`, replacing any previous copy atomically.
async fn materialize_one(tool: Tool, dir: PathBuf) -> std::io::Result<()> {
    let destination = dir.join(tool.name);
    let staging = dir.join(format!(".{}.{}.tmp", tool.name, std::process::id()));

    tokio::fs::copy(tool.path, &staging).await?;
    tokio::fs::rename(&staging, &destination).await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_binaries_are_built() {
        for tool in bundled() {
            assert!(
                std::path::Path::new(tool.path).is_file(),
                "{} was not built at {}",
                tool.name,
                tool.path
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
                tokio::fs::read(tool.path).await.unwrap(),
                tokio::fs::read(&path).await.unwrap(),
                "{} does not match its source binary",
                tool.name
            );
        }
    }

    #[test]
    fn materialized_dir_is_stable() {
        assert_eq!(materialized_dir(), materialized_dir());
    }
}
