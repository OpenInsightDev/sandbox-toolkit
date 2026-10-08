//! The specification models each server entry as "exactly one of a closed set
//! of variants" — which is to say, a Rust enum. [`Server`] carries only the
//! fields its transport defines, so an entry mixing stdio and HTTP fields is
//! unrepresentable here and is rejected during parsing.
//!
//! Failure boundaries follow the specification: a top-level problem disables
//! MCP for the package ([`DisabledReason`]); a per-entry problem skips that
//! one server ([`ServerInvalid`]) and the rest keep loading.

use std::fmt;
use std::net::IpAddr;
use std::path::Path;

use serde_json::{Map, Value};

use crate::diag::{Diagnostic, Origin, Rule};
use crate::path::{RelativePath, lexically_contained};
use crate::spec::SpecVersion;
use crate::template::{Placeholder, Segment, Template};

pub const MCP_SCHEMA_1_0_0: &str = "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json";

/// A validated `mcp.json`: the servers that survived per-entry validation, in
/// declaration order.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Config {
    pub servers: Vec<ServerEntry>,
}

impl Config {
    pub fn server(&self, name: &str) -> Option<&Server> {
        self.servers.iter().find(|entry| entry.name == name).map(|entry| &entry.server)
    }

    pub fn parse(
        bytes: &[u8],
        manifest_spec: SpecVersion,
    ) -> Result<(Self, Vec<Diagnostic>), DisabledReason> {
        let document: Value = serde_json::from_slice(bytes)
            .map_err(|e| DisabledReason::NotJson { detail: e.to_string() })?;
        let Value::Object(object) = document else {
            return Err(DisabledReason::NotAnObject);
        };

        match object.get("$schema") {
            Some(Value::String(id)) => {
                // The MCP `$schema` must declare the same Agent Plugins version
                // as `plugin.json`. With one recognized version, that means
                // exactly this identifier.
                let matches = match manifest_spec {
                    SpecVersion::V1_0_0 => id == MCP_SCHEMA_1_0_0,
                };
                if !matches {
                    return Err(DisabledReason::SchemaMismatch { declared: id.clone() });
                }
            }
            _ => return Err(DisabledReason::MissingSchema),
        }

        if let Some(field) = object.keys().find(|k| *k != "$schema" && *k != "mcpServers") {
            return Err(DisabledReason::UnexpectedField { field: field.clone() });
        }

        let Some(Value::Object(members)) = object.get("mcpServers") else {
            return Err(DisabledReason::MissingServers);
        };

        let mut servers = Vec::new();
        let mut diagnostics = Vec::new();
        for (name, value) in members {
            match parse_server(value) {
                Ok(server) => servers.push(ServerEntry { name: name.clone(), server }),
                Err(why) => diagnostics.push(Diagnostic::new(
                    Rule::ServerSkipped,
                    Origin::Server(name.clone()),
                    why.to_string(),
                )),
            }
        }
        Ok((Self { servers }, diagnostics))
    }
}

pub async fn load(
    path: impl AsRef<Path>,
    manifest_spec: SpecVersion,
) -> Result<(Config, Vec<Diagnostic>), DisabledReason> {
    let path = path.as_ref();
    match tokio::fs::metadata(path).await {
        Ok(meta) if !meta.is_file() => return Err(DisabledReason::NotAFile),
        Ok(_) => {}
        Err(error) => {
            return Err(DisabledReason::Unavailable { detail: error.to_string() });
        }
    }
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|error| DisabledReason::Unavailable { detail: error.to_string() })?;
    Config::parse(&bytes, manifest_spec)
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct ServerEntry {
    pub name: String,
    pub server: Server,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Server {
    /// `"type": "stdio"`
    Stdio(StdioServer),
    /// `"type": "streamable-http"`
    StreamableHttp(RemoteServer),
    /// `"type": "sse"`, deprecated in MCP.
    Sse(RemoteServer),
}

impl Server {
    pub fn transport(&self) -> Transport {
        match self {
            Self::Stdio(_) => Transport::Stdio,
            Self::StreamableHttp(_) => Transport::StreamableHttp,
            Self::Sse(_) => Transport::Sse,
        }
    }
}

/// The three declared transports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Transport {
    Stdio,
    StreamableHttp,
    Sse,
}

impl fmt::Display for Transport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Stdio => "stdio",
            Self::StreamableHttp => "streamable-http",
            Self::Sse => "sse",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct StdioServer {
    pub command: Command,
    /// Each subject to placeholder expansion.
    pub args: Vec<Template>,
    /// Values subject to placeholder expansion. The names `PLUGIN_ROOT` and
    /// `PLUGIN_DATA` are reserved and rejected at parse time.
    pub env: Vec<(String, Template)>,
    /// Defaults to the package root when omitted.
    pub cwd: Cwd,
}

/// The `command` field: one executable token, never a shell string, never
/// expanded.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Command {
    /// Contains no path separators — a bare name is a *name*.
    Bare(String),
    /// A package-relative path to an executable bundled in the package.
    Bundled(RelativePath),
}

impl Command {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Bare(name) => name,
            Self::Bundled(path) => path.as_str(),
        }
    }
}

/// The working directory of a stdio server, in the three permitted forms.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Cwd {
    /// `cwd` omitted, or exactly `${PLUGIN_ROOT}`: the package root.
    PluginRoot,
    /// A package-relative path beginning with `./`.
    Relative(RelativePath),
    /// `${PLUGIN_ROOT}/subpath`, the subpath lexically contained.
    RootAnchored(String),
    /// Exactly `${PLUGIN_DATA}`, or `${PLUGIN_DATA}/subpath` with the subpath
    /// lexically contained in the data directory.
    DataAnchored(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct RemoteServer {
    pub url: Url,
    /// Fixed literal headers, in declaration order. Never expanded, never a
    /// secret mechanism.
    pub headers: Vec<(String, String)>,
}

/// An MCP endpoint URL: absolute `http`/`https`, no user information, no
/// fragment, and `http` only toward loopback.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Url {
    raw: String,
    https: bool,
    loopback: bool,
}

impl Url {
    pub fn parse(raw: &str) -> Result<Self, UrlInvalid> {
        if raw.contains('#') {
            return Err(UrlInvalid::HasFragment);
        }
        let (https, after_scheme) = if let Some(rest) = strip_scheme(raw, "https://") {
            (true, rest)
        } else if let Some(rest) = strip_scheme(raw, "http://") {
            (false, rest)
        } else {
            return Err(UrlInvalid::NotHttp);
        };
        let authority_end = after_scheme.find(['/', '?']).unwrap_or(after_scheme.len());
        let authority = &after_scheme[..authority_end];
        if authority.contains('@') {
            return Err(UrlInvalid::HasUserInfo);
        }
        let host = host_of(authority).ok_or(UrlInvalid::NoHost)?;
        let loopback = is_loopback_host(&host);
        if !https && !loopback {
            return Err(UrlInvalid::PlainHttpBeyondLoopback);
        }
        Ok(Self { raw: raw.to_owned(), https, loopback })
    }

    pub fn as_str(&self) -> &str {
        &self.raw
    }

    pub fn is_https(&self) -> bool {
        self.https
    }

    pub fn is_loopback(&self) -> bool {
        self.loopback
    }
}

impl fmt::Display for Url {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.raw)
    }
}

fn strip_scheme<'a>(raw: &'a str, scheme: &str) -> Option<&'a str> {
    (raw.len() >= scheme.len() && raw[..scheme.len()].eq_ignore_ascii_case(scheme))
        .then(|| &raw[scheme.len()..])
}

fn host_of(authority: &str) -> Option<String> {
    if let Some(rest) = authority.strip_prefix('[') {
        let end = rest.find(']')?;
        let host = &rest[..end];
        let port = &rest[end + 1..];
        if !port.is_empty() && !port.strip_prefix(':')?.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        return (!host.is_empty()).then(|| format!("[{host}]"));
    }
    let host = match authority.split_once(':') {
        Some((host, port)) => {
            if port.is_empty() || !port.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            host
        }
        None => authority,
    };
    (!host.is_empty()).then(|| host.to_owned())
}

fn is_loopback_host(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    let bare = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
    bare.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// Why a URL failed validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum UrlInvalid {
    NotHttp,
    HasUserInfo,
    HasFragment,
    NoHost,
    PlainHttpBeyondLoopback,
}

impl fmt::Display for UrlInvalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotHttp => "URL must be absolute http or https",
            Self::HasUserInfo => "URL must not contain user information",
            Self::HasFragment => "URL must not contain a fragment",
            Self::NoHost => "URL has no valid host",
            Self::PlainHttpBeyondLoopback => {
                "http is only permitted when the host is localhost or a loopback IP literal"
            }
        })
    }
}

impl std::error::Error for UrlInvalid {}

/// Why MCP was disabled for the whole package.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DisabledReason {
    NotAFile,
    LocationEscapes,
    Unavailable { detail: String },
    NotJson { detail: String },
    NotAnObject,
    MissingSchema,
    SchemaMismatch { declared: String },
    UnexpectedField { field: String },
    MissingServers,
}

impl fmt::Display for DisabledReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAFile => f.write_str("mcp.json is not a regular file"),
            Self::LocationEscapes => f.write_str("mcp.json resolves outside the package root"),
            Self::Unavailable { detail } => write!(f, "mcp.json could not be read: {detail}"),
            Self::NotJson { detail } => write!(f, "mcp.json is not valid JSON: {detail}"),
            Self::NotAnObject => f.write_str("mcp.json must contain a top-level JSON object"),
            Self::MissingSchema => f.write_str("required field `$schema` is missing"),
            Self::SchemaMismatch { declared } => {
                write!(f, "`$schema` declares an unsupported or mismatched version: {declared}")
            }
            Self::UnexpectedField { field } => {
                write!(f, "unexpected top-level field `{field}` in a closed schema")
            }
            Self::MissingServers => {
                f.write_str("required field `mcpServers` is missing or not an object")
            }
        }
    }
}

impl std::error::Error for DisabledReason {}

/// Why one server entry was skipped.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ServerInvalid {
    NotAnObject,
    MissingType,
    UnknownType { declared: String },
    ForeignField { field: String },
    WrongType {
        /// The offending field.
        field: &'static str,
        /// What the specification requires there.
        expected: &'static str,
    },
    NotAToken { command: String },
    CommandEscapes,
    BadCwd { cwd: String },
    ReservedEnv { name: String },
    BadHeaderName { name: String },
    BadHeaderValue { name: String },
    DuplicateHeader { name: String },
    BadUrl(UrlInvalid),
}

impl fmt::Display for ServerInvalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAnObject => f.write_str("server entry must be a JSON object"),
            Self::MissingType => f.write_str("required field `type` is missing"),
            Self::UnknownType { declared } => write!(f, "unknown transport type {declared:?}"),
            Self::ForeignField { field } => {
                write!(f, "field `{field}` does not belong to this transport variant")
            }
            Self::WrongType { field, expected } => write!(f, "field `{field}` must be {expected}"),
            Self::NotAToken { command } => write!(
                f,
                "`command` {command:?} must be a single bare executable name or a `./` path"
            ),
            Self::CommandEscapes => f.write_str("`command` resolves outside the package root"),
            Self::BadCwd { cwd } => write!(
                f,
                "`cwd` {cwd:?} must be `./…`, `${{PLUGIN_ROOT}}[/…]`, or `${{PLUGIN_DATA}}[/…]` \
                 and stay inside its anchor"
            ),
            Self::ReservedEnv { name } => {
                write!(f, "`env` must not set reserved variable `{name}`")
            }
            Self::BadHeaderName { name } => write!(f, "invalid HTTP header name {name:?}"),
            Self::BadHeaderValue { name } => write!(f, "invalid value for HTTP header {name:?}"),
            Self::DuplicateHeader { name } => {
                write!(f, "header {name:?} appears more than once (names are case-insensitive)")
            }
            Self::BadUrl(err) => write!(f, "invalid endpoint URL: {err}"),
        }
    }
}

impl std::error::Error for ServerInvalid {}

fn parse_server(value: &Value) -> Result<Server, ServerInvalid> {
    let Value::Object(fields) = value else {
        return Err(ServerInvalid::NotAnObject);
    };
    let declared = match fields.get("type") {
        Some(Value::String(t)) => t.as_str(),
        Some(_) => return Err(ServerInvalid::WrongType { field: "type", expected: "a string" }),
        None => return Err(ServerInvalid::MissingType),
    };
    match declared {
        "stdio" => parse_stdio(fields).map(Server::Stdio),
        "streamable-http" => parse_remote(fields).map(Server::StreamableHttp),
        "sse" => parse_remote(fields).map(Server::Sse),
        other => Err(ServerInvalid::UnknownType { declared: other.to_owned() }),
    }
}

fn parse_stdio(fields: &Map<String, Value>) -> Result<StdioServer, ServerInvalid> {
    for field in fields.keys() {
        if !matches!(field.as_str(), "type" | "command" | "args" | "env" | "cwd") {
            return Err(ServerInvalid::ForeignField { field: field.clone() });
        }
    }

    let command = match fields.get("command") {
        Some(Value::String(raw)) => parse_command(raw)?,
        Some(_) => return Err(ServerInvalid::WrongType { field: "command", expected: "a string" }),
        None => return Err(ServerInvalid::WrongType { field: "command", expected: "present" }),
    };

    let args = match fields.get("args") {
        None => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| match item {
                Value::String(s) => Ok(Template::parse(s)),
                _ => Err(ServerInvalid::WrongType { field: "args", expected: "an array of strings" }),
            })
            .collect::<Result<_, _>>()?,
        Some(_) => return Err(ServerInvalid::WrongType { field: "args", expected: "an array of strings" }),
    };

    let env = match fields.get("env") {
        None => Vec::new(),
        Some(Value::Object(entries)) => {
            let mut env = Vec::with_capacity(entries.len());
            for (name, value) in entries {
                if name == "PLUGIN_ROOT" || name == "PLUGIN_DATA" {
                    return Err(ServerInvalid::ReservedEnv { name: name.clone() });
                }
                let Value::String(raw) = value else {
                    return Err(ServerInvalid::WrongType {
                        field: "env",
                        expected: "an object of strings",
                    });
                };
                env.push((name.clone(), Template::parse(raw)));
            }
            env
        }
        Some(_) => return Err(ServerInvalid::WrongType { field: "env", expected: "an object of strings" }),
    };

    let cwd = match fields.get("cwd") {
        None => Cwd::PluginRoot,
        Some(Value::String(raw)) => parse_cwd(raw)?,
        Some(_) => return Err(ServerInvalid::WrongType { field: "cwd", expected: "a string" }),
    };

    Ok(StdioServer { command, args, env, cwd })
}

fn parse_command(raw: &str) -> Result<Command, ServerInvalid> {
    if raw.starts_with("./") {
        return RelativePath::parse(raw)
            .map(Command::Bundled)
            .map_err(|_| ServerInvalid::CommandEscapes);
    }
    if raw.is_empty() || raw.contains('/') || raw.contains('\\') {
        return Err(ServerInvalid::NotAToken { command: raw.to_owned() });
    }
    Ok(Command::Bare(raw.to_owned()))
}

/// A placeholder anywhere but the very start is none of the three permitted
/// forms.
fn parse_cwd(raw: &str) -> Result<Cwd, ServerInvalid> {
    let bad = || ServerInvalid::BadCwd { cwd: raw.to_owned() };
    let template = Template::parse(raw);
    match template.segments() {
        [Segment::Literal(_)] => RelativePath::parse(raw).map(Cwd::Relative).map_err(|_| bad()),
        [Segment::Anchor(Placeholder::PluginRoot)] => Ok(Cwd::PluginRoot),
        [Segment::Anchor(Placeholder::PluginData)] => Ok(Cwd::DataAnchored(String::new())),
        [Segment::Anchor(anchor), Segment::Literal(rest)] => {
            let subpath = rest.strip_prefix('/').ok_or_else(bad)?;
            if !lexically_contained(subpath) {
                return Err(bad());
            }
            Ok(match anchor {
                Placeholder::PluginRoot => Cwd::RootAnchored(subpath.to_owned()),
                Placeholder::PluginData => Cwd::DataAnchored(subpath.to_owned()),
            })
        }
        _ => Err(bad()),
    }
}

fn parse_remote(fields: &Map<String, Value>) -> Result<RemoteServer, ServerInvalid> {
    for field in fields.keys() {
        if !matches!(field.as_str(), "type" | "url" | "headers") {
            return Err(ServerInvalid::ForeignField { field: field.clone() });
        }
    }
    let url = match fields.get("url") {
        Some(Value::String(raw)) => Url::parse(raw).map_err(ServerInvalid::BadUrl)?,
        Some(_) => return Err(ServerInvalid::WrongType { field: "url", expected: "a string" }),
        None => return Err(ServerInvalid::WrongType { field: "url", expected: "present" }),
    };
    let headers = match fields.get("headers") {
        None => Vec::new(),
        Some(Value::Object(entries)) => {
            let mut headers: Vec<(String, String)> = Vec::with_capacity(entries.len());
            for (name, value) in entries {
                if !is_valid_header_name(name) {
                    return Err(ServerInvalid::BadHeaderName { name: name.clone() });
                }
                let Value::String(v) = value else {
                    return Err(ServerInvalid::WrongType {
                        field: "headers",
                        expected: "an object of strings",
                    });
                };
                if !is_valid_header_value(v) {
                    return Err(ServerInvalid::BadHeaderValue { name: name.clone() });
                }
                if headers.iter().any(|(seen, _)| seen.eq_ignore_ascii_case(name)) {
                    return Err(ServerInvalid::DuplicateHeader { name: name.clone() });
                }
                headers.push((name.clone(), v.clone()));
            }
            headers
        }
        Some(_) => {
            return Err(ServerInvalid::WrongType {
                field: "headers",
                expected: "an object of strings",
            });
        }
    };
    Ok(RemoteServer { url, headers })
}

/// RFC 9110 token characters, the legal alphabet of an HTTP field name.
fn is_valid_header_name(name: &str) -> bool {
    !name.is_empty()
        && name.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(
                    b,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

/// An HTTP field value: no control characters other than horizontal tab.
fn is_valid_header_value(value: &str) -> bool {
    value.bytes().all(|b| b == b'\t' || (b != 0x7f && b >= 0x20))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_rules() {
        assert!(Url::parse("https://deploy.example.com/mcp").is_ok());
        assert!(Url::parse("http://localhost:8080/mcp").unwrap().is_loopback());
        assert!(Url::parse("http://127.0.0.1/mcp").unwrap().is_loopback());
        assert!(Url::parse("http://[::1]:9000/sse").unwrap().is_loopback());
        assert_eq!(
            Url::parse("http://mcp.example.com/api"),
            Err(UrlInvalid::PlainHttpBeyondLoopback)
        );
        assert_eq!(Url::parse("https://a:b@example.com/"), Err(UrlInvalid::HasUserInfo));
        assert_eq!(Url::parse("https://example.com/api#frag"), Err(UrlInvalid::HasFragment));
        assert_eq!(Url::parse("ftp://example.com/"), Err(UrlInvalid::NotHttp));
        assert_eq!(Url::parse("https:///path"), Err(UrlInvalid::NoHost));
        assert!(Url::parse("http://127.8.9.1/x").unwrap().is_loopback());
    }

    #[test]
    fn command_rules() {
        assert_eq!(parse_command("npx"), Ok(Command::Bare("npx".into())));
        assert!(matches!(parse_command("./bin/server"), Ok(Command::Bundled(_))));
        assert!(matches!(parse_command("../bin/server"), Err(ServerInvalid::NotAToken { .. })));
        assert!(matches!(parse_command("bin/server"), Err(ServerInvalid::NotAToken { .. })));
        assert!(matches!(parse_command("./bin/../../server"), Err(ServerInvalid::CommandEscapes)));
        assert!(matches!(parse_command(""), Err(ServerInvalid::NotAToken { .. })));
    }

    #[test]
    fn cwd_forms() {
        assert!(matches!(parse_cwd("./data"), Ok(Cwd::Relative(_))));
        assert_eq!(parse_cwd("${PLUGIN_ROOT}"), Ok(Cwd::PluginRoot));
        assert_eq!(parse_cwd("${PLUGIN_ROOT}/x"), Ok(Cwd::RootAnchored("x".into())));
        assert_eq!(parse_cwd("${PLUGIN_DATA}"), Ok(Cwd::DataAnchored(String::new())));
        assert_eq!(parse_cwd("${PLUGIN_DATA}/x/y"), Ok(Cwd::DataAnchored("x/y".into())));
        assert!(parse_cwd("data").is_err());
        assert!(parse_cwd("${PLUGIN_ROOT}/../x").is_err());
        assert!(parse_cwd("./x/${PLUGIN_ROOT}").is_err());
        assert!(parse_cwd("${PLUGIN_ROOT}x").is_err());
    }

    const SCHEMA: &str = MCP_SCHEMA_1_0_0;

    #[test]
    fn per_entry_failures_do_not_disable_mcp() {
        let doc = format!(
            r#"{{"$schema": "{SCHEMA}",
                 "mcpServers": {{
                   "good": {{"type": "stdio", "command": "npx"}},
                   "bad": {{"type": "stdio"}}
                 }}}}"#
        );
        let (config, diagnostics) = Config::parse(doc.as_bytes(), SpecVersion::V1_0_0).unwrap();
        assert_eq!(config.servers.len(), 1);
        assert_eq!(config.servers[0].name, "good");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].rule, Rule::ServerSkipped);
    }

    #[test]
    fn a_bad_top_level_disables_mcp() {
        let err = Config::parse(b"not json", SpecVersion::V1_0_0).unwrap_err();
        assert!(matches!(err, DisabledReason::NotJson { .. }));
    }

    #[tokio::test]
    async fn load_reads_and_validates_a_file() {
        let directory = std::env::temp_dir().join(format!(
            "dotagents-mcp-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("the clock is past the epoch")
                .as_nanos()
        ));
        tokio::fs::create_dir_all(&directory).await.unwrap();
        let path = directory.join("mcp.json");
        let document = format!(
            r#"{{"$schema": "{SCHEMA}",
                 "mcpServers": {{"echo": {{"type": "stdio", "command": "npx"}}}}}}"#
        );
        tokio::fs::write(&path, document).await.unwrap();

        let (config, diagnostics) = load(&path, SpecVersion::V1_0_0).await.unwrap();
        assert_eq!(config.servers.len(), 1);
        assert!(diagnostics.is_empty());

        let _ = tokio::fs::remove_dir_all(&directory).await;
    }
}
