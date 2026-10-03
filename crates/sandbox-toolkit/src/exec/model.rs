use std::collections::HashMap;
use std::ffi::OsString;
use std::io;
use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use ts_rs::TS;

use crate::workspace::Metadata;

/// The payload kind a request carries.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ExecFormat {
    #[default]
    Exec,
    Shell,
}

#[derive(Debug, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub struct ExecRequest {
    #[serde(default)]
    pub format: ExecFormat,
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub wait: u64,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub script: Option<String>,
    #[serde(default)]
    pub shell: Option<String>,
}

impl ExecRequest {
    /// The program and the arguments that follow it; `shell` runs `script`
    /// through an interpreter.
    pub fn resolve(&self) -> Result<(String, Vec<String>), ExecError> {
        match self.format {
            ExecFormat::Exec => {
                let command = self.command.clone().ok_or(ExecError::Missing("command"))?;
                Ok((command, self.args.clone()))
            }
            ExecFormat::Shell => {
                let script = self.script.clone().ok_or(ExecError::Missing("script"))?;
                let shell = self.shell.clone().unwrap_or_else(|| "sh".to_owned());
                Ok((shell, vec!["-c".to_owned(), script]))
            }
        }
    }

    /// The whole environment the child starts with: the service's own, the
    /// workspace variable injected into it, and the request's entries overriding
    /// both.
    pub fn environment(&self, workspace: &Metadata) -> Vec<(OsString, OsString)> {
        let mut env: HashMap<OsString, OsString> = std::env::vars_os().collect();
        let (name, root) = workspace.env();
        env.insert(name.into(), root);
        env.extend(
            self.env
                .iter()
                .map(|(name, value)| (name.into(), value.into())),
        );
        env.into_iter().collect()
    }
}

/// How a command ended: it exited with a code, or a signal terminated it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema, TS)]
#[serde(tag = "status", rename_all = "snake_case")]
#[ts(export)]
pub enum ExecStatus {
    Exited { exit_code: i32 },
    Signaled { signal: i32 },
}

/// The direct answer to a command that finished within `wait`: its terminal
/// state plus the output captured up to that point.
#[derive(Debug, Serialize, JsonSchema, TS)]
#[serde(tag = "status", rename_all = "snake_case")]
#[ts(export)]
pub enum ExecResult {
    Exited {
        exit_code: i32,
        stdout: String,
        stderr: String,
    },
    Signaled {
        signal: i32,
        stdout: String,
        stderr: String,
    },
}

impl ExecResult {
    pub fn new(status: ExecStatus, stdout: String, stderr: String) -> Self {
        match status {
            ExecStatus::Exited { exit_code } => Self::Exited {
                exit_code,
                stdout,
                stderr,
            },
            ExecStatus::Signaled { signal } => Self::Signaled {
                signal,
                stdout,
                stderr,
            },
        }
    }
}

#[derive(Debug, Error)]
pub enum ExecError {
    #[error("`{0}` is required")]
    Missing(&'static str),
    #[error("failed to start `{program}`: {source}")]
    Spawn {
        program: String,
        #[source]
        source: io::Error,
    },
}
