use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, Read},
    path::Component,
};

use agent_plugins::{
    Diagnostic, LoadedPlugin, McpOutcome, MemSource, Rejection, ServerEntry, Skill, Transport,
};
use axum::body::Bytes;
use flate2::read::GzDecoder;
use thiserror::Error;

use crate::model::{
    PluginAuthor, PluginDiagnostic, PluginManifest, PluginMcp, PluginMcpServer, PluginMcpStatus,
    PluginParseResult, PluginRejection, PluginSkill, PluginTransport,
};

pub const MAX_ARCHIVE_BYTES: usize = 16 * 1024 * 1024;

const MAX_UNCOMPRESSED_BYTES: u64 = 64 * 1024 * 1024;

const MAX_ENTRIES: usize = 4_096;

#[derive(Debug, Error)]
pub enum PluginError {
    #[error("request body is empty; expected a gzip-compressed tar archive")]
    EmptyBody,
    #[error("archive exceeds the {MAX_ARCHIVE_BYTES}-byte compressed limit")]
    TooLarge,
    #[error("invalid plugin archive: {0}")]
    InvalidArchive(String),
    #[error("plugin parsing task failed")]
    Task(#[from] tokio::task::JoinError),
}

pub async fn parse(bytes: Bytes) -> Result<PluginParseResult, PluginError> {
    tokio::task::spawn_blocking(move || analyze(&bytes)).await?
}

fn analyze(bytes: &[u8]) -> Result<PluginParseResult, PluginError> {
    if bytes.is_empty() {
        return Err(PluginError::EmptyBody);
    }
    if bytes.len() > MAX_ARCHIVE_BYTES {
        return Err(PluginError::TooLarge);
    }

    let files = strip_single_root(read_archive(bytes)?);

    let mut source = MemSource::new();
    for (path, contents) in files {
        source = source.file(path, contents);
    }

    Ok(report(source.load()))
}

fn read_archive(bytes: &[u8]) -> Result<BTreeMap<String, Vec<u8>>, PluginError> {
    let mut archive = tar::Archive::new(GzDecoder::new(bytes));
    let mut files = BTreeMap::new();
    let mut remaining = MAX_UNCOMPRESSED_BYTES;

    for entry in archive.entries().map_err(invalid_archive)? {
        let entry = entry.map_err(invalid_archive)?;

        if !entry.header().entry_type().is_file() {
            continue;
        }
        if files.len() >= MAX_ENTRIES {
            return Err(PluginError::InvalidArchive(format!(
                "archive contains more than {MAX_ENTRIES} files"
            )));
        }

        let Some(path) = entry_path(&entry)? else {
            continue;
        };

        let mut contents = Vec::new();
        let read = entry
            .take(remaining + 1)
            .read_to_end(&mut contents)
            .map_err(invalid_archive)?;
        if read as u64 > remaining {
            return Err(PluginError::TooLarge);
        }
        remaining -= read as u64;

        files.insert(path, contents);
    }

    Ok(files)
}

fn entry_path<R: Read>(entry: &tar::Entry<'_, R>) -> Result<Option<String>, PluginError> {
    let path = entry.path().map_err(invalid_archive)?;

    let mut segments = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(segment) => match segment.to_str() {
                Some(segment) => segments.push(segment),
                None => {
                    return Err(PluginError::InvalidArchive(format!(
                        "archive entry path is not valid UTF-8: {}",
                        path.display()
                    )));
                }
            },
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(PluginError::InvalidArchive(format!(
                    "archive entry escapes the plugin root: {}",
                    path.display()
                )));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(PluginError::InvalidArchive(format!(
                    "archive entry uses an absolute path: {}",
                    path.display()
                )));
            }
        }
    }

    Ok((!segments.is_empty()).then(|| segments.join("/")))
}

fn strip_single_root(files: BTreeMap<String, Vec<u8>>) -> BTreeMap<String, Vec<u8>> {
    let mut roots: BTreeSet<String> = BTreeSet::new();
    let mut root_file = false;
    for path in files.keys() {
        match path.split_once('/') {
            Some((root, _)) => {
                roots.insert(root.to_string());
            }
            None => root_file = true,
        }
    }

    if root_file || roots.len() != 1 {
        return files;
    }

    let root = roots.into_iter().next().expect("one root");
    let prefix = format!("{root}/");
    if !files.contains_key(&format!("{prefix}plugin.json")) {
        return files;
    }

    files
        .into_iter()
        .map(|(path, contents)| (path[prefix.len()..].to_string(), contents))
        .collect()
}

fn report(outcome: Result<LoadedPlugin, Rejection>) -> PluginParseResult {
    match outcome {
        Ok(plugin) => PluginParseResult {
            valid: true,
            rejection: None,
            manifest: Some(manifest_report(&plugin)),
            skills: plugin.skills.iter().map(skill_report).collect(),
            mcp: Some(mcp_report(&plugin.mcp)),
            diagnostics: plugin.diagnostics.iter().map(diagnostic_report).collect(),
        },
        Err(rejection) => PluginParseResult {
            valid: false,
            rejection: Some(PluginRejection {
                code: rejection.code().to_string(),
                message: rejection.to_string(),
            }),
            manifest: None,
            skills: Vec::new(),
            mcp: None,
            diagnostics: Vec::new(),
        },
    }
}

fn manifest_report(plugin: &LoadedPlugin) -> PluginManifest {
    let manifest = &plugin.manifest;
    PluginManifest {
        spec_version: manifest.spec.to_string(),
        name: manifest.name.as_str().to_string(),
        version: manifest.version.clone(),
        description: manifest.description.clone(),
        author: manifest.author.as_ref().map(|author| PluginAuthor {
            name: author.name.clone(),
            email: author.email.clone(),
            url: author.url.clone(),
        }),
        homepage: manifest.homepage.clone(),
        repository: manifest.repository.clone(),
        license: manifest.license.clone(),
        keywords: manifest.keywords.clone(),
    }
}

fn skill_report(skill: &Skill) -> PluginSkill {
    PluginSkill {
        name: skill.meta.name.clone(),
        directory: skill.directory.clone(),
        path: skill.path.as_str().to_string(),
        description: skill.meta.description.clone(),
        license: skill.meta.license.clone(),
        compatibility: skill.meta.compatibility.clone(),
        allowed_tools: skill.meta.allowed_tools.clone(),
        metadata: skill.meta.metadata.clone(),
    }
}

fn mcp_report(outcome: &McpOutcome) -> PluginMcp {
    match outcome {
        McpOutcome::Absent => PluginMcp {
            status: PluginMcpStatus::Absent,
            reason: None,
            servers: Vec::new(),
        },
        McpOutcome::Disabled(reason) => PluginMcp {
            status: PluginMcpStatus::Disabled,
            reason: Some(reason.to_string()),
            servers: Vec::new(),
        },
        McpOutcome::Configured(config) => PluginMcp {
            status: PluginMcpStatus::Configured,
            reason: None,
            servers: config.servers.iter().map(server_report).collect(),
        },
        _ => PluginMcp {
            status: PluginMcpStatus::Unknown,
            reason: None,
            servers: Vec::new(),
        },
    }
}

fn server_report(entry: &ServerEntry) -> PluginMcpServer {
    PluginMcpServer {
        name: entry.name.clone(),
        transport: match entry.server.transport() {
            Transport::Stdio => PluginTransport::Stdio,
            Transport::StreamableHttp => PluginTransport::StreamableHttp,
            Transport::Sse => PluginTransport::Sse,
        },
    }
}

fn diagnostic_report(diagnostic: &Diagnostic) -> PluginDiagnostic {
    PluginDiagnostic {
        rule: diagnostic.rule.code().to_string(),
        section: diagnostic.rule.section().to_string(),
        origin: diagnostic.origin.to_string(),
        message: diagnostic.message.clone(),
    }
}

fn invalid_archive(error: io::Error) -> PluginError {
    PluginError::InvalidArchive(error.to_string())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use flate2::{Compression, write::GzEncoder};
    use serde_json::json;
    use tar::{Builder, Header};

    use super::*;

    const MANIFEST: &[u8] = br#"{
        "$schema": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
        "name": "demo-plugin",
        "version": "1.2.3"
    }"#;

    const MANIFEST_WITH_UNKNOWN_FIELD: &[u8] = br#"{
        "$schema": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
        "name": "demo-plugin",
        "hooks": {}
    }"#;

    const SKILL: &[u8] =
        b"---\nname: greet\ndescription: Greets. Use when greeting.\n---\nSay hi.\n";

    const MCP: &[u8] = br#"{
        "$schema": "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json",
        "mcpServers": { "echo": { "type": "stdio", "command": "npx" } }
    }"#;

    fn targz(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = Builder::new(Vec::new());
        for (path, contents) in entries {
            let mut header = Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(0o644);
            header.set_path(path).expect("fixture path is valid");
            header.set_cksum();
            builder
                .append_data(&mut header, path, *contents)
                .expect("appending fixture entry failed");
        }

        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(&builder.into_inner().expect("finishing fixture tar failed"))
            .expect("compressing fixture tar failed");
        encoder.finish().expect("finishing fixture gzip failed")
    }

    #[test]
    fn loads_a_plugin_with_skills_and_mcp() {
        let bytes = targz(&[
            ("plugin.json", MANIFEST),
            ("skills/greet/SKILL.md", SKILL),
            ("mcp.json", MCP),
        ]);

        let report = analyze(&bytes).expect("a valid archive should parse");

        assert!(report.valid);
        assert!(report.rejection.is_none());

        let manifest = report.manifest.expect("a valid plugin has a manifest");
        assert_eq!(manifest.name, "demo-plugin");
        assert_eq!(manifest.spec_version, "1.0.0");
        assert_eq!(manifest.version.as_deref(), Some("1.2.3"));

        assert_eq!(report.skills.len(), 1);
        assert_eq!(report.skills[0].name, "greet");
        assert_eq!(report.skills[0].path, "skills/greet");

        let mcp = report.mcp.expect("a valid plugin has an MCP outcome");
        assert_eq!(mcp.status, PluginMcpStatus::Configured);
        assert_eq!(mcp.servers.len(), 1);
        assert_eq!(mcp.servers[0].name, "echo");
        assert_eq!(mcp.servers[0].transport, PluginTransport::Stdio);

        assert!(report.diagnostics.is_empty());
    }

    #[test]
    fn reports_non_fatal_diagnostics() {
        let bytes = targz(&[("plugin.json", MANIFEST_WITH_UNKNOWN_FIELD)]);

        let report = analyze(&bytes).expect("an unknown field is not fatal");

        assert!(report.valid);
        assert_eq!(report.diagnostics.len(), 1);
        assert_eq!(report.diagnostics[0].rule, "unknown-manifest-field");
        assert_eq!(report.diagnostics[0].section, "§5.2");
        assert_eq!(report.diagnostics[0].origin, "plugin.json");
    }

    #[test]
    fn rejects_an_archive_without_a_manifest() {
        let bytes = targz(&[("README.md", b"hello")]);

        let report = analyze(&bytes).expect("the archive itself is well-formed");

        assert!(!report.valid);
        let rejection = report.rejection.expect("a rejection is reported");
        assert_eq!(rejection.code, "manifest-missing");
        assert!(report.manifest.is_none());
        assert!(report.mcp.is_none());
        assert!(report.skills.is_empty());
    }

    #[test]
    fn strips_a_single_top_level_directory() {
        let bytes = targz(&[
            ("demo-plugin/plugin.json", MANIFEST),
            ("demo-plugin/skills/greet/SKILL.md", SKILL),
        ]);

        let report = analyze(&bytes).expect("a wrapped package should parse");

        assert!(report.valid);
        assert_eq!(report.manifest.expect("manifest").name, "demo-plugin");
        assert_eq!(report.skills.len(), 1);
    }

    #[test]
    fn rejects_non_gzip_bodies() {
        let error = analyze(b"definitely not a gzip archive").unwrap_err();
        assert!(matches!(error, PluginError::InvalidArchive(_)));
    }

    #[test]
    fn rejects_an_empty_body() {
        let error = analyze(b"").unwrap_err();
        assert!(matches!(error, PluginError::EmptyBody));
    }

    #[test]
    fn rejects_absolute_entry_paths() {
        let mut builder = Builder::new(Vec::new());
        let mut header = Header::new_gnu();
        header.set_size(0);
        header.set_mode(0o644);
        header
            .set_path_absolute("/evil")
            .expect("absolute fixture path");
        header.set_cksum();
        builder
            .append(&header, &b""[..])
            .expect("appending fixture entry failed");

        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&builder.into_inner().unwrap()).unwrap();
        let bytes = encoder.finish().unwrap();

        let error = analyze(&bytes).unwrap_err();
        match error {
            PluginError::InvalidArchive(message) => assert!(message.contains("absolute path")),
            other => panic!("expected an invalid archive, got {other:?}"),
        }
    }

    #[test]
    fn serializes_with_camel_case_keys() {
        let bytes = targz(&[("plugin.json", MANIFEST)]);
        let report = analyze(&bytes).unwrap();

        let value = serde_json::to_value(&report).unwrap();
        assert_eq!(value["valid"], json!(true));
        assert_eq!(value["manifest"]["specVersion"], json!("1.0.0"));
        assert!(value.get("rejection").is_none());
        assert_eq!(value["mcp"]["status"], json!("absent"));
    }
}
