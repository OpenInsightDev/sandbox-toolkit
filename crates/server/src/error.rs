//! Maps the domain modules' typed errors onto HTTP and MCP responses, so the
//! two transports stay in sync.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use rmcp::ErrorData as McpError;

use crate::file::ReadFileError;

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

#[derive(Clone, Copy)]
enum ReadFileErrorKind {
    Invalid,
    NotFound,
    Internal,
}

impl ReadFileError {
    fn kind(&self) -> ReadFileErrorKind {
        match self {
            Self::RelativePath(_) | Self::ZeroLimit | Self::NotAFile(_) => {
                ReadFileErrorKind::Invalid
            }
            Self::NotFound(_) => ReadFileErrorKind::NotFound,
            Self::Io(_) => ReadFileErrorKind::Internal,
        }
    }
}

impl IntoResponse for ReadFileError {
    fn into_response(self) -> Response {
        let status = match self.kind() {
            ReadFileErrorKind::Invalid => StatusCode::BAD_REQUEST,
            ReadFileErrorKind::NotFound => StatusCode::NOT_FOUND,
            ReadFileErrorKind::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, self.to_string()).into_response()
    }
}

impl From<ReadFileError> for McpError {
    fn from(error: ReadFileError) -> Self {
        let message = error.to_string();
        match error.kind() {
            ReadFileErrorKind::Invalid => McpError::invalid_params(message, None),
            ReadFileErrorKind::NotFound => McpError::resource_not_found(message, None),
            ReadFileErrorKind::Internal => McpError::internal_error(message, None),
        }
    }
}
