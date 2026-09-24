//! Wire types shared by the file control and data plane.
//!
//! The same types also describe the MCP tools in [`super`], so their
//! `JsonSchema` derivations are the tool input and output schemas.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Captures shared by every file URI.
///
/// `path` is `None` only for the workspace root, which is addressed as
/// `/workspaces/{workspace_id}/fs` (or with a trailing slash) because a
/// wildcard capture never matches an empty path. The value is percent decoded once
/// by [`Path`], so the raw request target is kept through [`OriginalUri`] to
/// preserve the original percent encoding.
///
/// [`Path`]: axum::extract::Path
/// [`OriginalUri`]: axum::extract::OriginalUri
#[derive(Debug, Deserialize, JsonSchema, TS)]
#[ts(export)]
#[expect(dead_code, reason = "read by the file handlers, which are stubs")]
pub(crate) struct FilePath {
    workspace_id: String,
    #[serde(default)]
    path: Option<String>,
}

/// Text-line range requested when reading a file.
#[derive(Debug, Default, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct ReadFileRequest {
    /// Zero-based line offset.
    #[serde(default)]
    pub(crate) offset: usize,
    /// Maximum number of lines to return; omitted means through end of file.
    pub(crate) limit: Option<usize>,
}

/// A control-plane operation submitted as the `POST` body of a file URI.
///
/// Replaces the WebDAV `COPY` and `MOVE` methods: those are not standard HTTP
/// methods, so axum cannot route them, and their parameters are structured rather
/// than header-shaped. Whether the source version must match stays a request
/// header, see [`ConditionalHeaders`].
///
/// [`ConditionalHeaders`]: super::files::ConditionalHeaders
#[derive(Debug, Deserialize, JsonSchema, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case", tag = "operation")]
#[expect(dead_code, reason = "read by the file handlers, which are stubs")]
pub(crate) enum FileOperation {
    /// Duplicate the target file at `destination`.
    Copy {
        /// Workspace-relative path of the new file.
        destination: String,
        /// Whether an existing `destination` may be replaced. Absent means the
        /// target must not exist.
        #[serde(default)]
        overwrite: bool,
    },
    /// Move the target file to `destination`, removing it from `path`.
    Move {
        /// Workspace-relative path of the new file.
        destination: String,
        /// Whether an existing `destination` may be replaced. Absent means the
        /// target must not exist.
        #[serde(default)]
        overwrite: bool,
    },
}

/// Write parameters for a file: the target path plus its content.
#[derive(Debug, Deserialize, JsonSchema, TS)]
#[ts(export)]
#[expect(dead_code, reason = "read by the file handlers, which are stubs")]
pub(crate) struct WriteFileRequest {
    #[serde(flatten)]
    file: FilePath,
    /// New file content, replacing any existing content.
    content: String,
}

/// Copy or move parameters: the source path plus the operation to apply.
#[derive(Debug, Deserialize, JsonSchema, TS)]
#[ts(export)]
#[expect(dead_code, reason = "read by the file handlers, which are stubs")]
pub(crate) struct RelocateFileRequest {
    #[serde(flatten)]
    file: FilePath,
    #[serde(flatten)]
    operation: FileOperation,
}

#[derive(Debug, Serialize, JsonSchema, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FileType {
    File,
    Directory,
    Symlink,
}

#[derive(Debug, Serialize, JsonSchema, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub(crate) struct FileEntry {
    pub(crate) name: String,
    pub(crate) path: String,
    pub(crate) file_type: FileType,
    pub(crate) size: Option<u64>,
    pub(crate) etag: String,
    pub(crate) modified_at: String,
}

#[derive(Debug, Serialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct DirectoryResponse {
    pub(crate) path: String,
    pub(crate) entries: Vec<FileEntry>,
}

#[derive(Debug, Serialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct FileMetadata {
    pub(crate) path: String,
    pub(crate) file_type: FileType,
    pub(crate) size: Option<u64>,
    pub(crate) etag: String,
    pub(crate) modified_at: String,
}
