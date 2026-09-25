//! Wire types shared by the control and data plane.
//!
//! The same types also describe the MCP tools in [`super`], so their
//! `JsonSchema` derivations are the tool input and output schemas.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Captures shared by every resource URI.
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
#[expect(dead_code, reason = "read by the resource handlers, which are stubs")]
pub(crate) struct ResourcePath {
    workspace_id: String,
    #[serde(default)]
    path: Option<String>,
}

/// A control-plane operation submitted as the `POST` body of a resource URI.
///
/// Replaces the WebDAV `COPY` and `MOVE` methods: those are not standard HTTP
/// methods, so axum cannot route them, and their parameters are structured rather
/// than header-shaped. Whether the source version must match stays a request
/// header, see [`ConditionalHeaders`].
///
/// [`ConditionalHeaders`]: super::http::ConditionalHeaders
#[derive(Debug, Deserialize, JsonSchema, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case", tag = "operation")]
#[expect(dead_code, reason = "read by the resource handlers, which are stubs")]
pub(crate) enum ResourceOperation {
    /// Duplicate the target resource at `destination`.
    Copy {
        /// Workspace-relative path of the new resource.
        destination: String,
        /// Whether an existing `destination` may be replaced. Absent means the
        /// target must not exist.
        #[serde(default)]
        overwrite: bool,
    },
    /// Move the target resource to `destination`, removing it from `path`.
    Move {
        /// Workspace-relative path of the new resource.
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
#[expect(dead_code, reason = "read by the resource handlers, which are stubs")]
pub(crate) struct WriteFileRequest {
    #[serde(flatten)]
    resource: ResourcePath,
    /// New file content, replacing any existing content.
    content: String,
}

/// Copy or move parameters: the source path plus the operation to apply.
#[derive(Debug, Deserialize, JsonSchema, TS)]
#[ts(export)]
#[expect(dead_code, reason = "read by the resource handlers, which are stubs")]
pub(crate) struct RelocateResourceRequest {
    #[serde(flatten)]
    resource: ResourcePath,
    #[serde(flatten)]
    operation: ResourceOperation,
}

#[derive(Debug, PartialEq, Eq, Serialize, JsonSchema, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ResourceKind {
    File,
    Directory,
    Symlink,
}

#[derive(Debug, Serialize, JsonSchema, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub(crate) struct ResourceEntry {
    pub(crate) name: String,
    pub(crate) path: String,
    pub(crate) kind: ResourceKind,
    /// Content size in bytes; a directory or symlink carries no content and
    /// reports `0`.
    pub(crate) size: u64,
    pub(crate) etag: String,
    pub(crate) modified_at: String,
}

#[derive(Debug, Serialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct DirectoryResponse {
    pub(crate) path: String,
    pub(crate) entries: Vec<ResourceEntry>,
    /// Whether the server withheld entries past the returned slice.
    pub(crate) truncated: bool,
}

/// Everything one resource reports about itself.
#[derive(Debug, Serialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct ResourceMetadata {
    /// Final path component; empty for a root directory.
    pub(crate) name: String,
    /// Path in the request's addressing mode: workspace-relative, or remote
    /// absolute. Empty addresses the workspace root.
    pub(crate) path: String,
    pub(crate) kind: ResourceKind,
    /// Content size in bytes; a directory or symlink carries no content and
    /// reports `0`.
    pub(crate) size: u64,
    pub(crate) etag: String,
    /// Permission bits as an octal string, for example `"0644"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub(crate) mode: Option<String>,
    /// Owner user id.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub(crate) uid: Option<u64>,
    /// Owner group id.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub(crate) gid: Option<u64>,
    /// Inode number.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub(crate) inode: Option<u64>,
    /// Hard link count.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub(crate) links: Option<u64>,
    /// Device the resource resides on.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub(crate) device: Option<u64>,
    /// Device a special file points at.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub(crate) device_type: Option<u64>,
    /// Filesystem block size.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub(crate) block_size: Option<u64>,
    /// Number of blocks occupied.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub(crate) blocks: Option<u64>,
    /// Filesystem modification time, RFC 3339.
    pub(crate) modified_at: String,
    /// Filesystem access time, RFC 3339.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub(crate) accessed_at: Option<String>,
    /// Creation time, RFC 3339; omitted when the platform or filesystem does not
    /// record it.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub(crate) birthtime: Option<String>,
    /// Link target, verbatim as stored; only for `kind=symlink`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub(crate) target: Option<String>,
}
