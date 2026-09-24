//! Wire types of the process API exported to TypeScript.
//!
//! Their `TS` derivations are the client-side types.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

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
