use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::pty::TerminalSize;

/// The fields every format accepts, flattened into the selected variant so the
/// common shape is defined once.
#[derive(Debug, Deserialize, JsonSchema, TS)]
pub(crate) struct ExecCommon {
    /// Workspace-relative in workspace mode, absolute in direct mode. Defaults to
    /// the workspace root, or the server's directory in direct mode.
    #[serde(default)]
    pub(crate) cwd: Option<String>,
    /// A name a workspace also sets takes this value instead.
    #[serde(default)]
    pub(crate) env: HashMap<String, String>,
    /// How long the server waits before upgrading the response to a stream, in
    /// milliseconds; zero streams immediately and never terminates the command.
    #[serde(default)]
    pub(crate) wait: u64,
}

/// A closed union discriminated by `format`, so the payload that was selected
/// always has its required fields. The executable and its arguments are separate
/// tokens and never re-parsed by a shell, while a shell script is a single argument
/// to its interpreter.
#[derive(Debug, Deserialize, JsonSchema, TS)]
#[ts(export)]
#[serde(tag = "format", rename_all = "snake_case")]
pub(crate) enum ExecRequest {
    /// The default format; a body without a `format` tag is normalized to this
    /// variant before deserialization.
    Exec {
        /// A path, or a bare name resolved through `PATH`.
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(flatten)]
        #[ts(flatten)]
        common: ExecCommon,
    },
    Shell {
        /// The script handed to the interpreter as a single argument.
        script: String,
        /// The interpreter; `sh` when omitted. Resolved through `PATH`.
        shell: Option<String>,
        #[serde(flatten)]
        #[ts(flatten)]
        common: ExecCommon,
    },
}

impl ExecRequest {
    /// How long the server waits before upgrading the response to a stream.
    pub(crate) const fn wait(&self) -> u64 {
        match self {
            Self::Exec { common, .. } | Self::Shell { common, .. } => common.wait,
        }
    }
}

/// MCP has no route to carry addressing, so the workspace becomes a field, mirroring
/// the endpoint that mounts it.
#[derive(Debug, Deserialize, JsonSchema, TS)]
#[ts(export)]
#[expect(dead_code, reason = "read by the exec MCP tool, which is a stub")]
pub(crate) struct ExecToolRequest {
    /// Absent runs in direct mode, where `cwd` must be an absolute path.
    pub(crate) workspace_id: Option<String>,
    #[serde(flatten)]
    pub(crate) exec: ExecRequest,
}

/// The command is fixed at creation and streams over a WebSocket, so unlike exec and
/// shell there is no `wait`: the server owns the session lifetime.
#[derive(Debug, Deserialize, TS)]
#[ts(export)]
#[expect(
    dead_code,
    reason = "read by the pty session handlers, which are stubs"
)]
pub(crate) struct PtyRequest {
    pub(crate) command: String,
    #[serde(default)]
    pub(crate) args: Vec<String>,
    pub(crate) cwd: Option<String>,
    #[serde(default)]
    pub(crate) env: HashMap<String, String>,
    /// The runtime default when omitted.
    pub(crate) size: Option<TerminalSize>,
}

/// A structured object rather than plain text, so a further outcome, such as a
/// signal, is an added variant instead of a new channel. Exit code `0` is a normal
/// `exited`, not a separate success.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case", tag = "status")]
pub(crate) enum Status {
    Exited { exit_code: i32 },
    Failed { message: String },
}

/// Output is decoded as UTF-8, replacing invalid sequences; a client that needs the
/// exact bytes reads the exec frame stream instead. The status is flattened, so
/// `status` and its `exit_code`/`message` sit beside `stdout` and `stderr`.
#[derive(Debug, Serialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct ExecResult {
    #[serde(flatten)]
    #[ts(flatten)]
    pub(crate) status: Status,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub(crate) struct PtySession {
    pub(crate) id: String,
    /// Server-relative path.
    pub(crate) endpoint: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_payload_is_json() {
        assert_eq!(
            serde_json::to_value(Status::Exited { exit_code: 0 }).unwrap(),
            serde_json::json!({ "status": "exited", "exit_code": 0 })
        );
        assert_eq!(
            serde_json::to_value(Status::Exited { exit_code: 1 }).unwrap(),
            serde_json::json!({ "status": "exited", "exit_code": 1 })
        );
        assert_eq!(
            serde_json::to_value(Status::Failed {
                message: "terminated by a signal".to_owned()
            })
            .unwrap(),
            serde_json::json!({ "status": "failed", "message": "terminated by a signal" })
        );
    }

    #[test]
    fn result_flattens_the_status() {
        assert_eq!(
            serde_json::to_value(ExecResult {
                status: Status::Exited { exit_code: 1 },
                stdout: "out".to_owned(),
                stderr: "err".to_owned(),
            })
            .unwrap(),
            serde_json::json!({
                "status": "exited",
                "exit_code": 1,
                "stdout": "out",
                "stderr": "err",
            })
        );
    }
}
