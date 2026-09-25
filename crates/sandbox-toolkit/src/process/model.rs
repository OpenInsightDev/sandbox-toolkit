use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::pty::TerminalSize;

/// The executable and its arguments are separate tokens and never re-parsed by a
/// shell, which is what distinguishes exec from shell.
#[derive(Debug, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct ExecRequest {
    /// A path, or a bare name resolved through `PATH`.
    pub(crate) command: String,
    #[serde(default)]
    pub(crate) args: Vec<String>,
    /// Workspace-relative in workspace mode, absolute in direct mode. Defaults to
    /// the workspace root, or the server's directory in direct mode.
    pub(crate) cwd: Option<String>,
    /// A name a workspace also sets takes this value instead.
    #[serde(default)]
    pub(crate) env: HashMap<String, String>,
    /// How long the server waits before upgrading the response to a stream, in
    /// milliseconds; it bounds only the wait and never terminates the command.
    pub(crate) timeout: Option<u64>,
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

/// The script is a single argument to the interpreter, so unlike exec it is
/// tokenized by a shell rather than passed through verbatim.
#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub(crate) struct ShellRequest {
    pub(crate) script: String,
    pub(crate) cwd: Option<String>,
    #[serde(default)]
    pub(crate) env: HashMap<String, String>,
    /// Resolved through `PATH`; `sh` when omitted.
    pub(crate) shell: Option<String>,
    pub(crate) timeout: Option<u64>,
}

/// The command is fixed at creation and streams over a WebSocket, so unlike exec and
/// shell there is no `timeout`: the server owns the session lifetime.
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
/// signal or a timeout, is an added variant instead of a new channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case", tag = "status")]
pub(crate) enum Status {
    Success,
    Exited { code: i32 },
    Failed { message: String },
}

impl Status {
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "read by the process handlers, which are not written yet"
        )
    )]
    pub(crate) const fn exit_code(&self) -> Option<i32> {
        match self {
            Self::Exited { code } => Some(*code),
            Self::Success | Self::Failed { .. } => None,
        }
    }
}

/// Output is decoded as UTF-8, replacing invalid sequences; a client that needs the
/// exact bytes reads the exec frame stream instead.
#[derive(Debug, Serialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct ExecResult {
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
            serde_json::to_value(Status::Success).unwrap(),
            serde_json::json!({ "status": "success" })
        );
        assert_eq!(
            serde_json::to_value(Status::Exited { code: 1 }).unwrap(),
            serde_json::json!({ "status": "exited", "code": 1 })
        );
    }

    #[test]
    fn reports_the_exit_code() {
        assert_eq!(Status::Exited { code: 3 }.exit_code(), Some(3));
        assert_eq!(Status::Success.exit_code(), None);
        assert_eq!(
            Status::Failed {
                message: "spawn failed".to_owned()
            }
            .exit_code(),
            None
        );
    }
}
