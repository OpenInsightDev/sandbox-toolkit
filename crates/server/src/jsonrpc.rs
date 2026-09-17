//! JSON-RPC 2.0 wire protocol.
//!
//! The types here are the shared contract between the server and the
//! TypeScript clients it drives: each one derives [`serde`] for the wire
//! format and [`ts_rs::TS`] so that running `cargo test` regenerates the
//! matching declarations into the repository-level `generated/` directory
//! (configured by `TS_RS_EXPORT_DIR` in `.cargo/config.toml`).
//!
//! Regenerate the TypeScript with:
//!
//! ```bash
//! cargo test -p sandbox-toolkit export_bindings
//! ```

#![allow(dead_code)] // Not wired into request handlers yet.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

/// The value of the `jsonrpc` member.
///
/// Serialized as the string literal `"2.0"`, which the generated TypeScript
/// narrows the field to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum Version {
    #[serde(rename = "2.0")]
    V2,
}

/// A request identifier.
///
/// JSON-RPC allows a string or a number (fractional numbers are discouraged).
/// `null` is reserved for responses whose request could not be parsed, so it is
/// represented by `Option::None` rather than a variant here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(untagged)]
#[ts(export)]
pub enum RequestId {
    Number(i64),
    String(String),
}

/// A JSON-RPC request.
///
/// A request with no [`id`](Self::id) is a *notification*: the server performs
/// the work but must not send a response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Request {
    /// Always `"2.0"`.
    pub jsonrpc: Version,
    /// The name of the method to invoke.
    pub method: String,
    /// Parameters passed by position (array) or by name (object), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
    /// Correlates a response with this request; `None` for notifications.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<RequestId>,
}

/// A JSON-RPC response.
///
/// Exactly one of `result` and `error` is present. The untagged representation
/// surfaces that as a union of two shapes in TypeScript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(untagged)]
#[ts(export)]
pub enum Response {
    /// The call succeeded.
    Success {
        jsonrpc: Version,
        result: Value,
        id: RequestId,
    },
    /// The call failed.
    Failure {
        jsonrpc: Version,
        error: ErrorObject,
        /// `null` when the request id could not be determined.
        id: Option<RequestId>,
    },
}

/// A JSON-RPC error object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ErrorObject {
    /// A machine-readable error code; see [`error_code`].
    pub code: i32,
    /// A short, human-readable description of the error.
    pub message: String,
    /// Additional information about the error, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// Error codes defined by the JSON-RPC 2.0 specification.
///
/// The range `-32000..=-32099` is reserved for implementation-defined server
/// errors.
pub mod error_code {
    /// Invalid JSON was received.
    pub const PARSE_ERROR: i32 = -32700;
    /// The JSON sent is not a valid request object.
    pub const INVALID_REQUEST: i32 = -32600;
    /// The method does not exist or is not available.
    pub const METHOD_NOT_FOUND: i32 = -32601;
    /// Invalid method parameters.
    pub const INVALID_PARAMS: i32 = -32602;
    /// Internal JSON-RPC error.
    pub const INTERNAL_ERROR: i32 = -32603;
}
