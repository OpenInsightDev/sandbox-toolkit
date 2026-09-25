use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// `path` is `None` only for the workspace root, because a wildcard capture never
/// matches an empty path. It is percent decoded once by [`Path`], so the raw
/// request target is kept through [`OriginalUri`] to preserve the original
/// percent encoding.
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

/// WebDAV `COPY` and `MOVE` are not standard HTTP methods, so axum cannot route
/// them and their parameters are structured rather than header-shaped. Whether
/// the source version must match stays a request header.
#[derive(Debug, Deserialize, JsonSchema, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case", tag = "operation")]
#[expect(dead_code, reason = "read by the resource handlers, which are stubs")]
pub(crate) enum ResourceOperation {
    Copy {
        destination: String,
        /// Absent means the target must not exist.
        #[serde(default)]
        overwrite: bool,
    },
    Move {
        destination: String,
        /// Absent means the target must not exist.
        #[serde(default)]
        overwrite: bool,
    },
}

#[derive(Debug, Deserialize, JsonSchema, TS)]
#[ts(export)]
#[expect(dead_code, reason = "read by the resource handlers, which are stubs")]
pub(crate) struct WriteFileRequest {
    #[serde(flatten)]
    resource: ResourcePath,
    content: String,
}

#[derive(Debug, Deserialize, JsonSchema, TS)]
#[ts(export)]
#[expect(dead_code, reason = "read by the resource handlers, which are stubs")]
pub(crate) struct RelocateResourceRequest {
    #[serde(flatten)]
    resource: ResourcePath,
    #[serde(flatten)]
    operation: ResourceOperation,
}

/// The window and recursion of one directory listing.
#[derive(Debug, Default, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct ListRequest {
    #[serde(default)]
    pub(crate) offset: usize,
    /// Absent means the server's default page size.
    pub(crate) limit: Option<usize>,
    /// `infinity` walks the whole subtree; absent lists direct children.
    pub(crate) depth: Option<Depth>,
}

#[derive(Debug, PartialEq, Eq, Deserialize, JsonSchema, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Depth {
    Infinity,
}

/// The patterns and window of one glob search.
#[derive(Debug, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct GlobRequest {
    pub(crate) pattern: String,
    /// A match removes the hit and prunes the directory's subtree.
    #[serde(default)]
    pub(crate) exclude: Vec<String>,
    #[serde(default)]
    pub(crate) offset: usize,
    /// Absent means the server's default page size.
    pub(crate) limit: Option<usize>,
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
    /// `0` for a directory or symlink.
    pub(crate) size: u64,
    pub(crate) etag: String,
    pub(crate) modified_at: String,
}

#[derive(Debug, Serialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct DirectoryResponse {
    pub(crate) path: String,
    pub(crate) entries: Vec<ResourceEntry>,
    pub(crate) truncated: bool,
}

#[derive(Debug, Serialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct ResourceMetadata {
    /// Empty for a root directory.
    pub(crate) name: String,
    /// In the request's addressing mode: workspace-relative, or remote absolute.
    /// Empty addresses the workspace root.
    pub(crate) path: String,
    pub(crate) kind: ResourceKind,
    /// `0` for a directory or symlink.
    pub(crate) size: u64,
    pub(crate) etag: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub(crate) mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub(crate) uid: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub(crate) gid: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub(crate) inode: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub(crate) links: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub(crate) device: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub(crate) device_type: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub(crate) block_size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub(crate) blocks: Option<u64>,
    pub(crate) modified_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub(crate) accessed_at: Option<String>,
    /// Omitted when the platform or filesystem does not record it.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub(crate) birthtime: Option<String>,
    /// Verbatim as stored; only for `kind=symlink`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub(crate) target: Option<String>,
}
