use std::collections::HashMap;
use std::ffi::OsString;
use std::path::Path;
use std::path::PathBuf;

use pty::TerminalSize;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use ts_rs::TS;

use crate::workspace::Metadata;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub struct PtySize {
    pub rows: u16,
    pub cols: u16,
}

impl Default for PtySize {
    fn default() -> Self {
        TerminalSize::default().into()
    }
}

impl From<TerminalSize> for PtySize {
    fn from(size: TerminalSize) -> Self {
        Self {
            rows: size.rows,
            cols: size.cols,
        }
    }
}

impl From<PtySize> for TerminalSize {
    fn from(size: PtySize) -> Self {
        Self {
            rows: size.rows,
            cols: size.cols,
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub struct PtyRequest {
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub size: PtySize,
}

impl PtyRequest {
    pub fn resolve(&self, workspace: &Metadata, bin: &Path) -> Result<PtyProcess, PtyError> {
        let program = self.command.clone().ok_or(PtyError::Missing("command"))?;

        Ok(PtyProcess {
            program,
            args: self.args.clone(),
            cwd: self.cwd.clone().unwrap_or_else(|| workspace.root.clone()),
            env: workspace.child_env(bin, &self.env),
            size: self.size.into(),
        })
    }
}

pub struct PtyProcess {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(OsString, OsString)>,
    pub size: TerminalSize,
}

#[derive(Debug, Serialize, JsonSchema, TS)]
#[ts(export)]
pub struct PtySession {
    pub id: String,
    pub endpoint: String,
}

#[derive(Debug, Error)]
pub enum PtyError {
    #[error("`{0}` is required")]
    Missing(&'static str),
    #[error("failed to start `{program}`: {source}")]
    Spawn {
        program: String,
        #[source]
        source: anyhow::Error,
    },
}
