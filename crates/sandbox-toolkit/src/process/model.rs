//! Wire types of the process API exported to TypeScript.
//!
//! Their `TS` derivations are the client-side types.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Parameters of an exec, submitted as the `POST .../exec` body.
///
/// The executable and its arguments are separate tokens and never re-parsed by a
/// shell, which is what distinguishes exec from shell.
#[derive(Debug, Deserialize, TS)]
#[ts(export)]
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "read by the exec handler, which is not wired up yet"
    )
)]
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
}

/// How a command finished.
///
/// A structured object rather than plain text, so a further outcome, such as a
/// signal or a timeout, is an added variant instead of a new channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
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
