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
}
