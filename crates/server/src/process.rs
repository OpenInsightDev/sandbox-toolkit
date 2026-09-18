//! `process/exec` — the implementation shared by the HTTP and MCP adapters.

use std::{
    ffi::OsString,
    io,
    path::PathBuf,
    process::Stdio,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use thiserror::Error;
use tokio::{io::AsyncWriteExt, process::Command};

use crate::{
    model::{DEFAULT_MAX_OUTPUT, ExecParams, ExecResult},
    tools,
};

/// Why running a command failed.
#[derive(Debug, Error)]
pub enum ExecError {
    /// `command` was empty or only whitespace.
    #[error("command must not be empty")]
    EmptyCommand,
    /// `cwd` was present but not absolute.
    #[error("cwd must be absolute: {}", .0.display())]
    RelativeCwd(PathBuf),
    /// `limit` was present but not at least `1`.
    #[error("limit must be at least 1")]
    ZeroLimit,
    /// The command could not be started, for example because it or its working
    /// directory does not exist.
    #[error("failed to run `{command}`: {source}")]
    Spawn {
        /// The command that could not be started.
        command: String,
        #[source]
        source: io::Error,
    },
    /// Output could not be captured or the temporary output file could not be
    /// written.
    #[error("failed to capture command output: {0}")]
    Io(#[from] io::Error),
}

/// Run a command to completion and capture its output.
///
/// At most `limit` bytes are returned inline. Anything larger is written to a
/// temporary file whose path is returned as `output_path` instead.
pub async fn exec(params: &ExecParams) -> Result<ExecResult, ExecError> {
    if params.command.trim().is_empty() {
        return Err(ExecError::EmptyCommand);
    }

    if let Some(cwd) = &params.cwd {
        let path = PathBuf::from(cwd);
        if !path.is_absolute() {
            return Err(ExecError::RelativeCwd(path));
        }
    }

    let limit = params.limit.unwrap_or(DEFAULT_MAX_OUTPUT);
    if limit == 0 {
        return Err(ExecError::ZeroLimit);
    }

    let mut command = Command::new(&params.command);
    command
        .args(params.args.as_deref().unwrap_or_default())
        .stdin(Stdio::null())
        // Do not leave an orphan running when the request is cancelled.
        .kill_on_drop(true);
    if let Some(cwd) = &params.cwd {
        command.current_dir(cwd);
    }
    if let Some(env) = &params.env {
        command.envs(env);
    }

    let inherited_path = params
        .env
        .as_ref()
        .and_then(|env| env.get("PATH"))
        .map(OsString::from)
        .or_else(|| std::env::var_os("PATH"));
    command.env("PATH", tools::search_path(inherited_path.as_deref()));

    let output = command.output().await.map_err(|source| ExecError::Spawn {
        command: params.command.clone(),
        source,
    })?;

    summarize(output.stdout, output.stderr, output.status.code(), limit).await
}

/// Build the result, keeping at most `limit` output bytes inline and spilling
/// anything larger to a temporary file.
async fn summarize(
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    exit_code: Option<i32>,
    limit: usize,
) -> Result<ExecResult, ExecError> {
    if stdout.len() + stderr.len() <= limit {
        return Ok(ExecResult {
            exit_code,
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
            truncated: false,
            output_path: None,
        });
    }

    // Spend the inline budget on standard output first, then on standard error.
    let mut budget = limit;
    let stdout_preview = take_preview(&stdout, &mut budget);
    let stderr_preview = take_preview(&stderr, &mut budget);

    let output_path = write_temp_output(&stdout, &stderr).await?;

    Ok(ExecResult {
        exit_code,
        stdout: stdout_preview,
        stderr: stderr_preview,
        truncated: true,
        output_path: Some(output_path.to_string_lossy().into_owned()),
    })
}

/// Decode up to `budget` bytes of `bytes`, decreasing `budget` by what was used.
fn take_preview(bytes: &[u8], budget: &mut usize) -> String {
    let taken = bytes.len().min(*budget);
    *budget -= taken;
    String::from_utf8_lossy(&bytes[..taken]).into_owned()
}

static OUTPUT_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Write `stdout` followed by `stderr` to a fresh, owner-readable temp file.
async fn write_temp_output(stdout: &[u8], stderr: &[u8]) -> io::Result<PathBuf> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let path = std::env::temp_dir().join(format!(
        "sandbox-toolkit-exec-{}-{nanos}-{}.log",
        std::process::id(),
        OUTPUT_COUNTER.fetch_add(1, Ordering::Relaxed),
    ));

    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .await?;
    file.write_all(stdout).await?;
    file.write_all(stderr).await?;
    file.flush().await?;

    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(command: &str) -> ExecParams {
        ExecParams {
            command: command.to_string(),
            args: None,
            cwd: None,
            env: None,
            limit: None,
        }
    }

    #[tokio::test]
    async fn captures_stdout_and_exit_code() {
        let mut params = params("printf");
        params.args = Some(vec!["hello".to_string()]);

        let result = exec(&params).await.unwrap();

        assert_eq!(result.stdout, "hello");
        assert_eq!(result.stderr, "");
        assert_eq!(result.exit_code, Some(0));
        assert!(!result.truncated);
        assert_eq!(result.output_path, None);
    }

    #[tokio::test]
    async fn captures_stderr_and_a_failing_exit_code() {
        let mut params = params("sh");
        params.args = Some(vec![
            "-c".to_string(),
            "printf oops >&2; exit 3".to_string(),
        ]);

        let result = exec(&params).await.unwrap();

        assert_eq!(result.stdout, "");
        assert_eq!(result.stderr, "oops");
        assert_eq!(result.exit_code, Some(3));
    }

    #[tokio::test]
    async fn applies_cwd_and_env() {
        let mut params = params("sh");
        params.args = Some(vec![
            "-c".to_string(),
            "printf %s \"$SANDBOX_EXEC_TEST\"".to_string(),
        ]);
        params.cwd = Some(std::env::temp_dir().to_string_lossy().into_owned());
        params.env = Some(std::collections::BTreeMap::from([(
            "SANDBOX_EXEC_TEST".to_string(),
            "from-env".to_string(),
        )]));

        let result = exec(&params).await.unwrap();

        assert_eq!(result.stdout, "from-env");
    }

    #[tokio::test]
    async fn prepends_the_tools_directory_to_path() {
        let mut params = params("sh");
        params.args = Some(vec!["-c".to_string(), "printf %s \"$PATH\"".to_string()]);

        let result = exec(&params).await.unwrap();

        let mut entries = std::env::split_paths(&result.stdout);
        assert_eq!(
            entries.next().as_deref(),
            Some(crate::tools::materialized_dir().as_path())
        );
    }

    #[tokio::test]
    async fn prepends_the_tools_directory_to_a_caller_supplied_path() {
        let mut params = params("/bin/sh");
        params.args = Some(vec!["-c".to_string(), "printf %s \"$PATH\"".to_string()]);
        params.env = Some(std::collections::BTreeMap::from([(
            "PATH".to_string(),
            "/custom/bin".to_string(),
        )]));

        let result = exec(&params).await.unwrap();

        let entries: Vec<_> = std::env::split_paths(&result.stdout)
            .map(|entry| entry.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            entries,
            vec![
                crate::tools::materialized_dir()
                    .to_string_lossy()
                    .into_owned(),
                "/custom/bin".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn output_over_limit_is_written_to_a_file() {
        let mut params = params("printf");
        params.args = Some(vec!["%s".to_string(), "x".repeat(500).to_string()]);
        params.limit = Some(10);

        let result = exec(&params).await.unwrap();

        assert!(result.truncated);
        assert_eq!(result.stdout, "x".repeat(10));
        assert_eq!(result.stderr, "");

        let path = result.output_path.expect("a truncated result has a file");
        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "x".repeat(500));
        std::fs::remove_file(&path).unwrap();
    }

    #[tokio::test]
    async fn output_at_the_limit_is_returned_inline() {
        let mut params = params("printf");
        params.args = Some(vec!["%s".to_string(), "x".repeat(10).to_string()]);
        params.limit = Some(10);

        let result = exec(&params).await.unwrap();

        assert!(!result.truncated);
        assert_eq!(result.stdout, "x".repeat(10));
        assert_eq!(result.output_path, None);
    }

    #[tokio::test]
    async fn rejects_an_empty_command() {
        let error = exec(&params("  ")).await.unwrap_err();
        assert!(matches!(error, ExecError::EmptyCommand));
    }

    #[tokio::test]
    async fn rejects_a_relative_cwd() {
        let mut params = params("true");
        params.cwd = Some("relative/dir".to_string());

        let error = exec(&params).await.unwrap_err();

        assert!(matches!(error, ExecError::RelativeCwd(_)));
    }

    #[tokio::test]
    async fn rejects_a_zero_limit() {
        let mut params = params("true");
        params.limit = Some(0);

        let error = exec(&params).await.unwrap_err();

        assert!(matches!(error, ExecError::ZeroLimit));
    }

    #[tokio::test]
    async fn reports_a_missing_command_as_a_spawn_error() {
        let error = exec(&params("definitely-not-a-real-command"))
            .await
            .unwrap_err();

        assert!(matches!(error, ExecError::Spawn { .. }));
    }
}
