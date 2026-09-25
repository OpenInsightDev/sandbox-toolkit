#![expect(
    dead_code,
    reason = "read and built by the filesystem handlers, which are stubs"
)]

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ResourceKind {
    File,
    Directory,
    Symlink,
}

#[derive(Debug, Serialize, JsonSchema, TS)]
#[ts(export)]
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
pub(crate) struct ResourceMetadata {
    /// Empty for the addressed root.
    pub(crate) name: String,
    /// In the request's addressing mode: workspace-relative or remote absolute.
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

/// The `entries` and `truncated` shape shared by `QUERY ?type=list` and
/// `QUERY ?type=glob`.
#[derive(Debug, Serialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct DirectoryResponse {
    pub(crate) entries: Vec<ResourceEntry>,
    pub(crate) truncated: bool,
}

#[derive(Debug, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct ContentRequest {
    pub(crate) path: String,
    /// Absent lets the server choose.
    pub(crate) encoding: Option<ContentEncoding>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[ts(export)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ContentEncoding {
    Utf8,
    Base64,
}

#[derive(Debug, Serialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct ContentResponse {
    pub(crate) path: String,
    pub(crate) encoding: ContentEncoding,
    pub(crate) content: String,
    pub(crate) size: u64,
    pub(crate) etag: String,
}

#[derive(Debug, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct StreamRequest {
    pub(crate) path: String,
}

#[derive(Debug, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct MetadataRequest {
    pub(crate) path: String,
}

/// The window and recursion of one directory listing.
#[derive(Debug, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct ListRequest {
    /// Empty addresses the workspace root.
    pub(crate) path: String,
    /// `infinity` walks the whole subtree; absent lists direct children.
    pub(crate) depth: Option<Depth>,
    /// Absent starts at the first entry.
    pub(crate) offset: Option<u64>,
    /// Absent uses the server's page size.
    pub(crate) limit: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Depth {
    Infinity,
}

#[derive(Debug, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct GlobRequest {
    pub(crate) path: String,
    pub(crate) pattern: String,
    /// A match removes the hit and prunes its subtree.
    #[serde(default)]
    pub(crate) exclude: Vec<String>,
    pub(crate) offset: Option<u64>,
    pub(crate) limit: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct RealpathRequest {
    pub(crate) path: String,
}

#[derive(Debug, Serialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct RealpathResponse {
    pub(crate) path: String,
}

#[derive(Debug, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct AccessRequest {
    pub(crate) path: String,
}

#[derive(Debug, Serialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct AccessResponse {
    pub(crate) readable: bool,
    pub(crate) writable: bool,
    pub(crate) executable: bool,
}

#[derive(Debug, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct LinesRequest {
    pub(crate) path: String,
    /// Zero-based first line; absent starts at the first.
    pub(crate) offset: Option<u64>,
    /// Absent uses the server's page size.
    pub(crate) limit: Option<u64>,
}

#[derive(Debug, Serialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct LinesResponse {
    pub(crate) lines: Vec<String>,
    pub(crate) offset: u64,
    pub(crate) truncated: bool,
}

#[derive(Debug, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct WatchRequest {
    pub(crate) path: String,
    /// Absent watches direct children only.
    pub(crate) recursive: Option<bool>,
}

/// One line of the NDJSON `QUERY ?type=watch` stream.
#[derive(Debug, Serialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct WatchEvent {
    pub(crate) event: WatchEventKind,
    pub(crate) path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WatchEventKind {
    Create,
    Update,
    Remove,
}

#[derive(Debug, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct WriteFileRequest {
    pub(crate) path: String,
    pub(crate) encoding: ContentEncoding,
    pub(crate) content: String,
}

#[derive(Debug, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct CreateDirectoryRequest {
    pub(crate) path: String,
    /// Absent creates only the final directory.
    pub(crate) recursive: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct CreateSymlinkRequest {
    pub(crate) path: String,
    /// Verbatim; may dangle.
    pub(crate) target: String,
}

/// Rejecting unknown fields is what keeps the immutable attributes out.
#[derive(Debug, Deserialize, JsonSchema, TS)]
#[ts(export)]
#[serde(deny_unknown_fields)]
pub(crate) struct PatchMetadataRequest {
    pub(crate) path: String,
    /// Permission bits, octal.
    pub(crate) mode: Option<String>,
    /// RFC 3339.
    pub(crate) modified_at: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct ApplyPatchRequest {
    pub(crate) path: String,
    pub(crate) format: PatchFormat,
    pub(crate) patch: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PatchFormat {
    Unified,
}

#[derive(Debug, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct TruncateRequest {
    pub(crate) path: String,
    pub(crate) length: u64,
}

#[derive(Debug, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct CopyRequest {
    pub(crate) path: String,
    pub(crate) destination: String,
}

#[derive(Debug, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct MoveRequest {
    pub(crate) path: String,
    pub(crate) destination: String,
}

#[derive(Debug, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub(crate) struct DeleteRequest {
    pub(crate) path: String,
    /// Absent deletes only a file or symlink.
    pub(crate) recursive: Option<bool>,
    /// Absent reports a missing target.
    pub(crate) force: Option<bool>,
}
