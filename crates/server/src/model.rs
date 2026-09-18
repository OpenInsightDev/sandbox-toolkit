//! Wire types shared by the HTTP endpoints, the TypeScript SDK and the MCP tool schema.

use std::collections::BTreeMap;

use rmcp::schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Parameters for the `tools/describe` operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
#[ts(export)]
pub struct DescribeToolParams {
    /// Name of a bundled tool, for example `fd`.
    pub name: String,
}

/// Result of the `tools/describe` operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
#[ts(export)]
pub struct DescribeToolResult {
    /// Name the executable is invoked as.
    pub name: String,
    /// Path the executable is materialized to at runtime.
    pub path: String,
}

/// Result of the `tools/list` operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
#[ts(export)]
pub struct ListToolsResult {
    /// Every executable embedded in this build.
    pub tools: Vec<DescribeToolResult>,
}

/// Result of the `health` operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
#[ts(export)]
pub struct HealthResult {
    /// Liveness marker, always `"ok"` when the server responds.
    pub status: String,
    /// Seconds since the server started.
    pub uptime_seconds: u64,
}

/// Number of lines returned by `fs/readFile` when `limit` is omitted.
pub const DEFAULT_LIMIT: usize = 2_000;

/// Parameters for the `fs/readFile` operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
#[ts(export)]
pub struct ReadFileParams {
    /// Absolute path of the text file to read.
    pub path: String,
    /// Zero-based index of the first line to return. Defaults to `0`, the first
    /// line of the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
    /// Maximum number of lines to return. Defaults to [`DEFAULT_LIMIT`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

/// Number of output bytes returned by `process/exec` when `limit` is omitted.
/// Larger output is written to a temporary file instead.
pub const DEFAULT_MAX_OUTPUT: usize = 64 * 1024;

/// Parameters for the `process/exec` operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
#[ts(export)]
pub struct ExecParams {
    /// Executable to run. A bare name is resolved against the inherited `PATH`.
    pub command: String,
    /// Arguments passed to the command, in order. Defaults to none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    /// Absolute working directory for the command. Defaults to the server's
    /// working directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Environment variables added on top of the server's environment,
    /// overriding it on conflict. Defaults to none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<BTreeMap<String, String>>,
    /// Maximum number of output bytes returned inline. Defaults to
    /// [`DEFAULT_MAX_OUTPUT`]; larger output is written to a temporary file and
    /// only its path is returned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

/// Result of the `process/exec` operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
#[ts(export)]
pub struct ExecResult {
    /// Exit status of the command, or `null` when a signal terminated it.
    pub exit_code: Option<i32>,
    /// Standard output, decoded lossily and capped at `limit` bytes.
    pub stdout: String,
    /// Standard error, decoded lossily and capped at `limit` bytes.
    pub stderr: String,
    /// Whether the output exceeded `limit` and was written to `output_path`.
    pub truncated: bool,
    /// Path of the temporary file holding the full output (standard output
    /// followed by standard error), present only when `truncated` is true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_path: Option<String>,
}

/// A single line of a text file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
#[ts(export)]
pub struct TextLine {
    /// One-based line number, matching what editors display.
    pub number: usize,
    /// The line's contents, without its trailing newline.
    pub text: String,
}

/// Result of the `fs/readFile` operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
#[ts(export)]
pub struct ReadFileResult {
    /// The path that was read, echoed back for correlation.
    pub path: String,
    /// The lines in the requested window, in file order.
    pub lines: Vec<TextLine>,
    /// Whether the file has more lines beyond `lines`.
    pub truncated: bool,
    /// Zero-based offset to pass to the next call to continue after this
    /// window, or absent once the end of the file has been reached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<usize>,
}

/// The kind of entry a [`Resource`] describes, the analog of the WebDAV
/// `DAV:resourcetype` live property.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS, utoipa::ToSchema,
)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
#[ts(export)]
pub enum ResourceKind {
    /// A regular file.
    File,
    /// A collection, the toolkit's name for a directory.
    Directory,
    /// A symbolic link, reported instead of following it to its target.
    Symlink,
}

/// Metadata for a single file, directory or symbolic link, the toolkit's
/// analog of the WebDAV live properties returned by `PROPFIND`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
#[ts(export)]
pub struct Resource {
    /// Absolute path of the entry.
    pub path: String,
    /// Last component of the path, the analog of `DAV:displayname`.
    pub name: String,
    /// Whether the entry is a file, directory or symbolic link.
    pub kind: ResourceKind,
    /// Size in bytes, the analog of `DAV:getcontentlength`. Directories report
    /// whatever size the platform records for them.
    pub size: u64,
    /// Last modification time in milliseconds since the Unix epoch, the analog
    /// of `DAV:getlastmodified`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified_at: Option<i64>,
    /// Creation time in milliseconds since the Unix epoch, the analog of
    /// `DAV:creationdate`, when the platform records one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<i64>,
    /// Whether the entry denies write permission to everyone.
    pub read_only: bool,
    /// Target of a symbolic link, present only when `kind` is `symlink`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

/// Parameters for the `fs/stat` operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
#[ts(export)]
pub struct StatParams {
    /// Absolute path of the entry to describe.
    pub path: String,
}

/// Result of the `fs/stat` operation, the analog of a `PROPFIND` with
/// `Depth: 0`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
#[ts(export)]
pub struct StatResult {
    /// Metadata for the described entry.
    pub resource: Resource,
}

/// Parameters for the `fs/list` operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
#[ts(export)]
pub struct ListParams {
    /// Absolute path of the directory to list.
    pub path: String,
}

/// Result of the `fs/list` operation, the analog of a `PROPFIND` with
/// `Depth: 1`. Only the collection's immediate members are returned; the
/// collection itself is described by [`Resource`] separately.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
#[ts(export)]
pub struct ListResult {
    /// The directory that was listed, echoed back for correlation.
    pub path: String,
    /// Immediate members, sorted by name.
    pub entries: Vec<Resource>,
}

/// Parameters for the `fs/mkdir` operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
#[ts(export)]
pub struct MkdirParams {
    /// Absolute path of the directory to create.
    pub path: String,
    /// Whether to create missing parents too. Defaults to `false`, matching
    /// `MKCOL`'s requirement that intermediate collections already exist.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recursive: Option<bool>,
}

/// Result of the `fs/mkdir` operation, the analog of `MKCOL`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
#[ts(export)]
pub struct MkdirResult {
    /// Metadata for the collection that was created.
    pub resource: Resource,
}

/// Parameters for the `fs/writeFile` operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
#[ts(export)]
pub struct WriteFileParams {
    /// Absolute path of the file to write.
    pub path: String,
    /// UTF-8 text to write. The file is created when missing and replaced
    /// otherwise, matching `PUT`.
    pub contents: String,
    /// Whether to append to the existing contents instead of replacing them.
    /// Defaults to `false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub append: Option<bool>,
}

/// Result of the `fs/writeFile` operation, the analog of `PUT`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
#[ts(export)]
pub struct WriteFileResult {
    /// Metadata for the file that was written.
    pub resource: Resource,
}

/// Parameters for the `fs/remove` operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
#[ts(export)]
pub struct RemoveParams {
    /// Absolute path of the file, symbolic link or directory to remove.
    pub path: String,
    /// Whether to remove a non-empty directory and its contents. Defaults to
    /// `false`, which refuses to delete a non-empty collection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recursive: Option<bool>,
}

/// Result of the `fs/remove` operation, the analog of `DELETE`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
#[ts(export)]
pub struct RemoveResult {
    /// The path that was removed, echoed back for correlation.
    pub path: String,
}

/// Parameters for the `fs/copy` operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
#[ts(export)]
pub struct CopyParams {
    /// Absolute path of the file, symbolic link or directory to copy.
    pub source: String,
    /// Absolute path to copy it to. Directories are copied recursively, with
    /// symbolic links recreated rather than followed.
    pub destination: String,
    /// Whether to replace an existing destination. Defaults to `false`, which
    /// reports a conflict instead of overwriting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overwrite: Option<bool>,
}

/// Result of the `fs/copy` operation, the analog of `COPY`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
#[ts(export)]
pub struct CopyResult {
    /// Metadata for the copy that was created.
    pub resource: Resource,
}

/// Parameters for the `fs/move` operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
#[ts(export)]
pub struct MoveParams {
    /// Absolute path of the file, symbolic link or directory to move.
    pub source: String,
    /// Absolute path to move it to.
    pub destination: String,
    /// Whether to replace an existing destination. Defaults to `false`, which
    /// reports a conflict instead of overwriting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overwrite: Option<bool>,
}

/// Result of the `fs/move` operation, the analog of `MOVE`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
#[ts(export)]
pub struct MoveResult {
    /// Metadata for the entry at its new location.
    pub resource: Resource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PluginParseResult {
    pub valid: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejection: Option<PluginRejection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<PluginManifest>,
    pub skills: Vec<PluginSkill>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp: Option<PluginMcp>,
    pub diagnostics: Vec<PluginDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PluginRejection {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PluginManifest {
    pub spec_version: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<PluginAuthor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    pub keywords: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PluginAuthor {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PluginSkill {
    pub name: String,
    pub directory: String,
    pub path: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compatibility: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<String>,
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PluginMcp {
    pub status: PluginMcpStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub servers: Vec<PluginMcpServer>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PluginMcpStatus {
    Absent,
    Disabled,
    Configured,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PluginMcpServer {
    pub name: String,
    pub transport: PluginTransport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, utoipa::ToSchema)]
#[serde(rename_all = "kebab-case")]
#[ts(export)]
pub enum PluginTransport {
    Stdio,
    StreamableHttp,
    Sse,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PluginDiagnostic {
    pub rule: String,
    pub section: String,
    pub origin: String,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use rmcp::schemars::schema_for;
    use serde_json::json;

    use super::*;

    #[test]
    fn params_schema_is_an_object_requiring_name() {
        let schema = serde_json::to_value(schema_for!(DescribeToolParams)).unwrap();
        assert_eq!(
            schema,
            json!({
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "title": "DescribeToolParams",
                "description": "Parameters for the `tools/describe` operation.",
                "type": "object",
                "properties": {
                    "name": {
                        "description": "Name of a bundled tool, for example `fd`.",
                        "type": "string"
                    }
                },
                "required": ["name"]
            })
        );
    }

    #[test]
    fn read_file_offset_and_limit_are_optional() {
        let schema = serde_json::to_value(schema_for!(ReadFileParams)).unwrap();
        assert_eq!(schema["required"], json!(["path"]));
        // `Option<usize>` is nullable, so both optional fields admit `null`.
        assert_eq!(
            schema["properties"]["limit"]["type"],
            json!(["integer", "null"])
        );
        assert_eq!(schema["properties"]["offset"]["minimum"], 0);
    }

    #[test]
    fn exec_params_require_only_the_command() {
        let schema = serde_json::to_value(schema_for!(ExecParams)).unwrap();
        assert_eq!(schema["required"], json!(["command"]));
        assert_eq!(
            schema["properties"]["args"]["type"],
            json!(["array", "null"])
        );
        assert_eq!(schema["properties"]["limit"]["minimum"], 0);
    }

    #[test]
    fn result_round_trips_as_json() {
        let result = DescribeToolResult {
            name: "fd".into(),
            path: "/tmp/fd".into(),
        };
        let json = serde_json::to_string(&result).unwrap();
        assert_eq!(json, r#"{"name":"fd","path":"/tmp/fd"}"#);
        assert_eq!(
            serde_json::from_str::<DescribeToolResult>(&json).unwrap(),
            result
        );
    }

    #[test]
    fn resource_kinds_serialize_to_lowercase_names() {
        let names: Vec<String> = [
            ResourceKind::File,
            ResourceKind::Directory,
            ResourceKind::Symlink,
        ]
        .into_iter()
        .map(|kind| serde_json::to_string(&kind).unwrap())
        .collect();

        assert_eq!(names, vec![r#""file""#, r#""directory""#, r#""symlink""#]);
    }

    #[test]
    fn resource_serializes_live_properties_with_camel_case_keys() {
        let resource = Resource {
            path: "/tmp/a.txt".into(),
            name: "a.txt".into(),
            kind: ResourceKind::File,
            size: 12,
            modified_at: Some(1_700_000_000_000),
            created_at: None,
            read_only: false,
            target: None,
        };

        let value = serde_json::to_value(&resource).unwrap();
        assert_eq!(value["kind"], json!("file"));
        assert_eq!(value["modifiedAt"], json!(1_700_000_000_000i64));
        assert_eq!(value["readOnly"], json!(false));
        assert!(value.get("createdAt").is_none());
        assert!(value.get("target").is_none());
    }

    #[test]
    fn mutation_optionals_are_not_required() {
        let mkdir = serde_json::to_value(schema_for!(MkdirParams)).unwrap();
        assert_eq!(mkdir["required"], json!(["path"]));

        let remove = serde_json::to_value(schema_for!(RemoveParams)).unwrap();
        assert_eq!(remove["required"], json!(["path"]));

        let copy = serde_json::to_value(schema_for!(CopyParams)).unwrap();
        assert_eq!(copy["required"], json!(["source", "destination"]));

        let write = serde_json::to_value(schema_for!(WriteFileParams)).unwrap();
        assert_eq!(write["required"], json!(["path", "contents"]));
    }
}
