use std::borrow::Cow;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Stable machine-readable classification; the code strings follow tusd so
/// clients see the same `ERR_*` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    UnsupportedVersion,
    MaxSizeExceeded,
    InvalidContentType,
    InvalidUploadLength,
    InvalidOffset,
    UploadNotFound,
    UploadLocked,
    LockTimeout,
    MismatchedOffset,
    UploadSizeExceeded,
    NotImplemented,
    UploadNotFinished,
    InvalidConcat,
    ConcatenationUnsupported,
    ModifyFinal,
    InternalServerError,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UnsupportedVersion => "ERR_UNSUPPORTED_VERSION",
            Self::MaxSizeExceeded => "ERR_MAX_SIZE_EXCEEDED",
            Self::InvalidContentType => "ERR_INVALID_CONTENT_TYPE",
            Self::InvalidUploadLength => "ERR_INVALID_UPLOAD_LENGTH",
            Self::InvalidOffset => "ERR_INVALID_OFFSET",
            Self::UploadNotFound => "ERR_UPLOAD_NOT_FOUND",
            Self::UploadLocked => "ERR_UPLOAD_LOCKED",
            Self::LockTimeout => "ERR_LOCK_TIMEOUT",
            Self::MismatchedOffset => "ERR_MISMATCHED_OFFSET",
            Self::UploadSizeExceeded => "ERR_UPLOAD_SIZE_EXCEEDED",
            Self::NotImplemented => "ERR_NOT_IMPLEMENTED",
            Self::UploadNotFinished => "ERR_UPLOAD_NOT_FINISHED",
            Self::InvalidConcat => "ERR_INVALID_CONCAT",
            Self::ConcatenationUnsupported => "ERR_CONCATENATION_UNSUPPORTED",
            Self::ModifyFinal => "ERR_MODIFY_FINAL",
            Self::InternalServerError => "ERR_INTERNAL_SERVER_ERROR",
        }
    }

    pub fn status(self) -> StatusCode {
        match self {
            Self::UnsupportedVersion => StatusCode::PRECONDITION_FAILED,
            Self::MaxSizeExceeded | Self::UploadSizeExceeded => StatusCode::PAYLOAD_TOO_LARGE,
            Self::InvalidContentType
            | Self::InvalidUploadLength
            | Self::InvalidOffset
            | Self::UploadNotFinished
            | Self::InvalidConcat
            | Self::ConcatenationUnsupported => StatusCode::BAD_REQUEST,
            Self::UploadNotFound => StatusCode::NOT_FOUND,
            Self::UploadLocked => StatusCode::LOCKED,
            Self::LockTimeout | Self::InternalServerError => StatusCode::INTERNAL_SERVER_ERROR,
            Self::MismatchedOffset => StatusCode::CONFLICT,
            Self::NotImplemented => StatusCode::NOT_IMPLEMENTED,
            Self::ModifyFinal => StatusCode::FORBIDDEN,
        }
    }
}

#[derive(Debug)]
pub struct Error {
    code: ErrorCode,
    message: Cow<'static, str>,
}

impl Error {
    pub fn new(code: ErrorCode, message: impl Into<Cow<'static, str>>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn code(&self) -> ErrorCode {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn unsupported_version() -> Self {
        Self::new(
            ErrorCode::UnsupportedVersion,
            "missing, invalid or unsupported Tus-Resumable header",
        )
    }

    pub fn max_size_exceeded() -> Self {
        Self::new(ErrorCode::MaxSizeExceeded, "maximum size exceeded")
    }

    pub fn invalid_content_type() -> Self {
        Self::new(
            ErrorCode::InvalidContentType,
            "missing or invalid Content-Type header",
        )
    }

    pub fn invalid_upload_length() -> Self {
        Self::new(
            ErrorCode::InvalidUploadLength,
            "missing or invalid Upload-Length header",
        )
    }

    pub fn invalid_offset() -> Self {
        Self::new(
            ErrorCode::InvalidOffset,
            "missing or invalid Upload-Offset header",
        )
    }

    pub fn not_found() -> Self {
        Self::new(ErrorCode::UploadNotFound, "upload not found")
    }

    pub fn file_locked() -> Self {
        Self::new(ErrorCode::UploadLocked, "file currently locked")
    }

    pub fn lock_timeout() -> Self {
        Self::new(
            ErrorCode::LockTimeout,
            "failed to acquire lock before timeout",
        )
    }

    pub fn mismatch_offset() -> Self {
        Self::new(ErrorCode::MismatchedOffset, "mismatched offset")
    }

    pub fn size_exceeded() -> Self {
        Self::new(ErrorCode::UploadSizeExceeded, "upload's size exceeded")
    }

    pub fn not_implemented() -> Self {
        Self::new(ErrorCode::NotImplemented, "feature not implemented")
    }

    pub fn upload_not_finished() -> Self {
        Self::new(
            ErrorCode::UploadNotFinished,
            "one of the partial uploads is not finished",
        )
    }

    pub fn invalid_concat() -> Self {
        Self::new(ErrorCode::InvalidConcat, "invalid Upload-Concat header")
    }

    pub fn concatenation_unsupported() -> Self {
        Self::new(
            ErrorCode::ConcatenationUnsupported,
            "Upload-Concat header is not supported by server",
        )
    }

    pub fn modify_final() -> Self {
        Self::new(
            ErrorCode::ModifyFinal,
            "modifying a final upload is not allowed",
        )
    }

    pub fn internal(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ErrorCode::InternalServerError, message)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for Error {}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let body = format!("{}: {}\n", self.code.as_str(), self.message);
        (
            self.code.status(),
            [(
                axum::http::header::CONTENT_TYPE,
                "text/plain; charset=utf-8",
            )],
            body,
        )
            .into_response()
    }
}
