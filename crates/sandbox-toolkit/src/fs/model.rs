use std::os::unix::fs::MetadataExt;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Deserialize)]
pub struct PathRequest {
    pub path: String,
}

/// The `depth` field: a level count, or `"infinity"` for every level below.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(untagged)]
pub enum Depth {
    Count(u64),
    Infinity(Infinite),
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub enum Infinite {
    #[serde(rename = "infinity")]
    Yes,
}

#[derive(Debug, Deserialize)]
pub struct ListRequest {
    pub path: String,
    pub depth: Option<Depth>,
    pub offset: Option<u64>,
    pub limit: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct GlobRequest {
    pub path: String,
    pub pattern: String,
    #[serde(default)]
    pub exclude: Vec<String>,
    pub offset: Option<u64>,
    pub limit: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct LinesRequest {
    pub path: String,
    pub offset: Option<u64>,
    pub limit: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct AccessRequest {
    pub path: String,
    pub ok: Option<bool>,
    pub readable: Option<bool>,
    pub writable: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct WatchRequest {
    pub path: String,
    pub recursive: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct Content {
    pub path: String,
    pub content: String,
    pub size: u64,
}

#[derive(Debug, Serialize)]
pub struct Stat {
    pub kind: &'static str,
    pub size: u64,
    pub modified_at: String,
    pub accessed_at: Option<String>,
    pub birthtime: Option<String>,
    pub mode: String,
    pub device: u64,
    pub device_type: u64,
    pub inode: u64,
    pub links: u64,
    pub uid: u32,
    pub gid: u32,
    pub block_size: u64,
    pub blocks: u64,
}

impl Stat {
    pub fn new(metadata: &std::fs::Metadata) -> Self {
        let file_type = metadata.file_type();
        let kind = if file_type.is_symlink() {
            "symlink"
        } else if file_type.is_dir() {
            "directory"
        } else {
            "file"
        };

        Self {
            kind,
            size: metadata.len(),
            modified_at: optional(metadata.modified()).unwrap_or_default(),
            accessed_at: optional(metadata.accessed()),
            birthtime: optional(metadata.created()),
            mode: format!("{:o}", metadata.mode() & 0o7777),
            device: metadata.dev(),
            device_type: metadata.rdev(),
            inode: metadata.ino(),
            links: metadata.nlink(),
            uid: metadata.uid(),
            gid: metadata.gid(),
            block_size: metadata.blksize(),
            blocks: metadata.blocks(),
        }
    }
}

fn optional(time: std::io::Result<SystemTime>) -> Option<String> {
    time.ok().map(timestamp)
}

fn timestamp(time: SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(time).to_rfc3339()
}

#[derive(Debug, Serialize)]
pub struct Entries {
    pub entries: Vec<Entry>,
}

#[derive(Debug, Serialize)]
pub struct Entry {
    pub path: String,
}

impl Entries {
    pub fn new(paths: Vec<String>) -> Self {
        let entries = paths.into_iter().map(|path| Entry { path }).collect();
        Self { entries }
    }
}

#[derive(Debug, Serialize)]
pub struct Lines {
    pub lines: Vec<String>,
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
pub struct RealPath {
    pub path: String,
}

/// One line of a `watch` response; `event` is `create`, `update` or `remove`.
#[derive(Debug, Serialize)]
pub struct Change {
    pub event: &'static str,
    pub path: String,
}

#[derive(Debug, Error)]
pub enum FsError {
    #[error("`{0}` was not found")]
    NotFound(String),
    #[error("permission denied on `{0}`")]
    PermissionDenied(String),
    #[error("`{0}` is not valid UTF-8")]
    NotUtf8(String),
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Watch(#[from] sandbox_toolkit_utils::watch::Error),
}
