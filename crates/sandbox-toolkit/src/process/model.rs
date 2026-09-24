//! Wire types of the process API exported to TypeScript.
//!
//! Their `TS` derivations are the client-side types and their `JsonSchema`
//! derivations describe the MCP exec tool.

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::pty::TerminalSize;

/// Parameters of an exec, submitted as the `POST .../exec` body.
///
/// The executable and its arguments are separate tokens and never re-parsed by a
/// shell, which is what distinguishes exec from shell.
#[derive(Debug, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct ExecRequest {
    /// Executable to run: a path, or a bare name resolved through `PATH`.
    pub(crate) command: String,
    /// Arguments passed verbatim, one argv entry each.
    #[serde(default)]
    pub(crate) args: Vec<String>,
    /// Working directory: workspace-relative in workspace mode, absolute in direct
    /// mode. Defaults to the workspace root, or the server's directory in direct mode.
    pub(crate) cwd: Option<String>,
    /// Variables layered over the inherited environment; a name a workspace also sets
    /// takes this value instead.
    #[serde(default)]
    pub(crate) env: HashMap<String, String>,
    /// How long the server waits for the command to finish before upgrading the response
    /// to a stream, in milliseconds; the server's default when omitted. It bounds only the
    /// wait for a result and never terminates the command.
    pub(crate) timeout: Option<u64>,
}

/// Parameters of the exec MCP tool.
///
/// MCP has no route to carry addressing, so the workspace a command runs in becomes a
/// field here, mirroring the endpoint that mounts it: a named workspace matches
/// `POST /workspaces/{workspace_id}/exec`, an absent one matches `POST /exec`. The
/// remaining parameters are the ones of [`ExecRequest`].
#[derive(Debug, Deserialize, JsonSchema, TS)]
#[ts(export)]
#[expect(dead_code, reason = "read by the exec MCP tool, which is a stub")]
pub(crate) struct ExecToolRequest {
    /// Workspace the command runs in; absent runs it in direct mode, where `cwd` must
    /// be an absolute path.
    pub(crate) workspace_id: Option<String>,
    /// The command and its environment, as in the `POST .../exec` body.
    #[serde(flatten)]
    pub(crate) exec: ExecRequest,
}

/// Parameters of a shell, submitted as the `POST .../shell` body.
///
/// The script is a single argument to the interpreter, so unlike exec it is tokenized
/// by a shell rather than verbatim.
#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub(crate) struct ShellRequest {
    /// Script content, parsed by the interpreter rather than passed through.
    pub(crate) script: String,
    /// Working directory, as in `ExecRequest`.
    pub(crate) cwd: Option<String>,
    /// Variables layered over the inherited environment, as in `ExecRequest`.
    #[serde(default)]
    pub(crate) env: HashMap<String, String>,
    /// Interpreter to run the script, resolved through `PATH`; `sh` when omitted.
    pub(crate) shell: Option<String>,
    /// Stream-upgrade wait, as in `ExecRequest::timeout`.
    pub(crate) timeout: Option<u64>,
}

/// Parameters of a pty session, submitted as the `POST .../pty` body.
///
/// The session's command is fixed at creation and streams over a WebSocket, so unlike
/// exec and shell there is no `timeout`: the server owns the session lifetime.
#[derive(Debug, Deserialize, TS)]
#[ts(export)]
#[expect(
    dead_code,
    reason = "read by the pty session handlers, which are stubs"
)]
pub(crate) struct PtyRequest {
    /// Command to run, usually an interactive shell.
    pub(crate) command: String,
    /// Arguments passed verbatim, as in `ExecRequest::args`.
    #[serde(default)]
    pub(crate) args: Vec<String>,
    /// Initial working directory, as in `ExecRequest::cwd`.
    pub(crate) cwd: Option<String>,
    /// Variables layered over the inherited environment, as in `ExecRequest::env`.
    #[serde(default)]
    pub(crate) env: HashMap<String, String>,
    /// Initial terminal geometry; the runtime default when omitted.
    pub(crate) size: Option<TerminalSize>,
}

/// How a command finished.
///
/// A structured object rather than plain text, so a further outcome, such as a
/// signal or a timeout, is an added variant instead of a new channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case", tag = "status")]
pub(crate) enum Status {
    /// The command exited with code 0.
    Success,
    /// The command exited with a non-zero code.
    Exited { code: i32 },
    /// The command could not run, or was terminated before it could exit.
    Failed { message: String },
}

impl Status {
    /// The exit code, when the command reached exit.
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

/// A command's outcome together with its output, returned when the response was not
/// upgraded to a stream.
///
/// Output is decoded as UTF-8, replacing invalid sequences; a client that needs the
/// exact bytes reads the exec frame stream instead.
#[derive(Debug, Serialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct ExecResult {
    /// How the command finished.
    pub(crate) status: Status,
    /// Everything the command wrote to stdout.
    pub(crate) stdout: String,
    /// Everything the command wrote to stderr.
    pub(crate) stderr: String,
}

/// A created pty session: the id addressing it and the WebSocket endpoint that attaches
/// to it.
///
/// Returned by `POST .../pty`; the client then opens `endpoint` and speaks the frames
/// of [`super::pty`].
#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub(crate) struct PtySession {
    /// Server-assigned session id.
    pub(crate) id: String,
    /// WebSocket endpoint to attach to, as a server-relative path.
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
