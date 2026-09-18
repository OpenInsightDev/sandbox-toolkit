//! Maps the domain modules' typed errors onto HTTP and MCP responses, so the
//! two transports stay in sync.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use rmcp::ErrorData as McpError;

use crate::{file::ReadFileError, plugins::PluginError, process::ExecError};

/// A tool name that matches no bundled executable.
#[derive(Debug, thiserror::Error)]
#[error("unknown tool: {0}")]
pub struct UnknownToolError(pub String);

impl IntoResponse for UnknownToolError {
    fn into_response(self) -> Response {
        (StatusCode::NOT_FOUND, self.to_string()).into_response()
    }
}

impl From<UnknownToolError> for McpError {
    fn from(error: UnknownToolError) -> Self {
        McpError::invalid_params(error.to_string(), None)
    }
}

/// Broad category an operation error falls into, shared by both transports.
#[derive(Clone, Copy)]
enum Kind {
    /// The request was malformed or named something that cannot be handled.
    Invalid,
    /// The requested resource does not exist.
    NotFound,
    /// The server failed while handling an otherwise valid request.
    Internal,
}

impl Kind {
    fn status(self) -> StatusCode {
        match self {
            Self::Invalid => StatusCode::BAD_REQUEST,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn mcp(self, message: String) -> McpError {
        match self {
            Self::Invalid => McpError::invalid_params(message, None),
            Self::NotFound => McpError::resource_not_found(message, None),
            Self::Internal => McpError::internal_error(message, None),
        }
    }
}

impl ReadFileError {
    fn kind(&self) -> Kind {
        match self {
            Self::RelativePath(_) | Self::ZeroLimit | Self::NotAFile(_) => Kind::Invalid,
            Self::NotFound(_) => Kind::NotFound,
            Self::Io(_) => Kind::Internal,
        }
    }
}

impl IntoResponse for ReadFileError {
    fn into_response(self) -> Response {
        (self.kind().status(), self.to_string()).into_response()
    }
}

impl From<ReadFileError> for McpError {
    fn from(error: ReadFileError) -> Self {
        error.kind().mcp(error.to_string())
    }
}

impl ExecError {
    fn kind(&self) -> Kind {
        match self {
            // A command or working directory that cannot be started is a bad
            // request, not a server fault.
            Self::EmptyCommand | Self::RelativeCwd(_) | Self::ZeroLimit | Self::Spawn { .. } => {
                Kind::Invalid
            }
            Self::Io(_) => Kind::Internal,
        }
    }
}

impl IntoResponse for ExecError {
    fn into_response(self) -> Response {
        (self.kind().status(), self.to_string()).into_response()
    }
}

impl From<ExecError> for McpError {
    fn from(error: ExecError) -> Self {
        error.kind().mcp(error.to_string())
    }
}

impl IntoResponse for PluginError {
    fn into_response(self) -> Response {
        let status = match &self {
            Self::EmptyBody | Self::InvalidArchive(_) => StatusCode::BAD_REQUEST,
            Self::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::Task(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, self.to_string()).into_response()
    }
}
