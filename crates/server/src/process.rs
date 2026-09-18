//! `process/exec` and `process/shell` — the implementation shared by the HTTP
//! and MCP adapters.

use std::{
    collections::BTreeMap,
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
    model::{DEFAULT_MAX_OUTPUT, DEFAULT_SHELL, ExecParams, ExecResult, ShellParams},
    tools,
};

/// Why running a command failed.
#[derive(Debug, Error)]
pub enum ExecError {
    /// `command` was empty or only whitespace.
    #[error("command must not be empty")]
    EmptyCommand,
    /// `script` was empty or only whitespace.
    #[error("script must not be empty")]
    EmptyScript,
    /// `shell` was present but empty or only whitespace.
    #[error("shell must not be empty")]
    EmptyShell,
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

    let cwd = validated_cwd(params.cwd.as_deref())?;
    let limit = validated_limit(params.limit)?;

    let mut command = Command::new(&params.command);
    command.args(params.args.as_deref().unwrap_or_default());
    configure(&mut command, cwd, params.env.as_ref());

    let output = command.output().await.map_err(|source| ExecError::Spawn {
        command: params.command.clone(),
        source,
    })?;

    summarize(output.stdout, output.stderr, output.status.code(), limit).await
}

/// Run a shell script to completion and capture its output.
///
/// The script is passed as a single string to `shell` via `-c`. As with
/// [`exec`], at most `limit` bytes are returned inline and anything larger is
/// written to a temporary file whose path is returned as `output_path`.
pub async fn shell(params: &ShellParams) -> Result<ExecResult, ExecError> {
    if params.script.trim().is_empty() {
        return Err(ExecError::EmptyScript);
    }

    let shell = params.shell.as_deref().unwrap_or(DEFAULT_SHELL);
    if shell.trim().is_empty() {
        return Err(ExecError::EmptyShell);
    }

    let cwd = validated_cwd(params.cwd.as_deref())?;
    let limit = validated_limit(params.limit)?;

    let mut command = Command::new(shell);
    command.arg("-c").arg(&params.script);
    configure(&mut command, cwd, params.env.as_ref());

    let output = command.output().await.map_err(|source| ExecError::Spawn {
        command: shell.to_string(),
        source,
    })?;

    summarize(output.stdout, output.stderr, output.status.code(), limit).await
}

/// Validate `cwd`, returning the borrowed value so callers can reuse it.
fn validated_cwd(cwd: Option<&str>) -> Result<Option<&str>, ExecError> {
    if let Some(cwd) = cwd {
        let path = PathBuf::from(cwd);
        if !path.is_absolute() {
            return Err(ExecError::RelativeCwd(path));
        }
    }
    Ok(cwd)
}

/// Resolve `limit`, defaulting to [`DEFAULT_MAX_OUTPUT`].
fn validated_limit(limit: Option<usize>) -> Result<usize, ExecError> {
    let limit = limit.unwrap_or(DEFAULT_MAX_OUTPUT);
    if limit == 0 {
        return Err(ExecError::ZeroLimit);
    }
    Ok(limit)
}

/// Apply the I/O, working directory, environment and `PATH` shared by every
/// spawned command.
fn configure(command: &mut Command, cwd: Option<&str>, env: Option<&BTreeMap<String, String>>) {
    command
        .stdin(Stdio::null())
        // Do not leave an orphan running when the request is cancelled.
        .kill_on_drop(true);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    if let Some(env) = env {
        command.envs(env);
    }

    let inherited_path = env
        .and_then(|env| env.get("PATH"))
        .map(OsString::from)
        .or_else(|| std::env::var_os("PATH"));
    command.env("PATH", tools::search_path(inherited_path.as_deref()));
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

    fn shell_params(script: &str) -> ShellParams {
        ShellParams {
            script: script.to_string(),
            cwd: None,
            env: None,
            shell: None,
            limit: None,
        }
    }

    #[tokio::test]
    async fn shell_runs_a_script_and_captures_output() {
        let result = shell(&shell_params("printf hello")).await.unwrap();

        assert_eq!(result.stdout, "hello");
        assert_eq!(result.stderr, "");
        assert_eq!(result.exit_code, Some(0));
        assert!(!result.truncated);
        assert_eq!(result.output_path, None);
    }

    #[tokio::test]
    async fn shell_defaults_to_sh() {
        let result = shell(&shell_params("printf %s \"$0\"")).await.unwrap();

        assert_eq!(result.stdout, DEFAULT_SHELL);
    }

    #[tokio::test]
    async fn shell_honors_a_custom_shell() {
        let mut params = shell_params("printf %s \"$0\"");
        params.shell = Some("/bin/sh".to_string());

        let result = shell(&params).await.unwrap();

        assert_eq!(result.stdout, "/bin/sh");
    }

    #[tokio::test]
    async fn shell_captures_stderr_and_a_failing_exit_code() {
        let result = shell(&shell_params("printf oops >&2; exit 3"))
            .await
            .unwrap();

        assert_eq!(result.stdout, "");
        assert_eq!(result.stderr, "oops");
        assert_eq!(result.exit_code, Some(3));
    }

    #[tokio::test]
    async fn shell_applies_cwd_and_env() {
        let mut params = shell_params("printf %s \"$SANDBOX_SHELL_TEST\"");
        params.cwd = Some(std::env::temp_dir().to_string_lossy().into_owned());
        params.env = Some(std::collections::BTreeMap::from([(
            "SANDBOX_SHELL_TEST".to_string(),
            "from-env".to_string(),
        )]));

        let result = shell(&params).await.unwrap();

        assert_eq!(result.stdout, "from-env");
    }

    #[tokio::test]
    async fn shell_output_over_limit_is_written_to_a_file() {
        let mut params = shell_params("printf %s \"xxxxxxxxxx\"");
        params.limit = Some(4);

        let result = shell(&params).await.unwrap();

        assert!(result.truncated);
        assert_eq!(result.stdout, "xxxx");

        let path = result.output_path.expect("a truncated result has a file");
        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "x".repeat(10));
        std::fs::remove_file(&path).unwrap();
    }

    #[tokio::test]
    async fn shell_rejects_an_empty_script() {
        let error = shell(&shell_params("  ")).await.unwrap_err();

        assert!(matches!(error, ExecError::EmptyScript));
    }

    #[tokio::test]
    async fn shell_rejects_an_empty_shell() {
        let mut params = shell_params("printf hello");
        params.shell = Some("  ".to_string());

        let error = shell(&params).await.unwrap_err();

        assert!(matches!(error, ExecError::EmptyShell));
    }

    #[tokio::test]
    async fn shell_rejects_a_relative_cwd() {
        let mut params = shell_params("printf hello");
        params.cwd = Some("relative/dir".to_string());

        let error = shell(&params).await.unwrap_err();

        assert!(matches!(error, ExecError::RelativeCwd(_)));
    }

    #[tokio::test]
    async fn shell_rejects_a_zero_limit() {
        let mut params = shell_params("printf hello");
        params.limit = Some(0);

        let error = shell(&params).await.unwrap_err();

        assert!(matches!(error, ExecError::ZeroLimit));
    }

    #[tokio::test]
    async fn shell_reports_a_missing_shell_as_a_spawn_error() {
        let mut params = shell_params("printf hello");
        params.shell = Some("definitely-not-a-real-shell".to_string());

        let error = shell(&params).await.unwrap_err();

        assert!(matches!(error, ExecError::Spawn { .. }));
    }
}
