//! Embeds the bundled command-line tools into the server as zstd payloads.
//!
//! Downloads are cached outside `target/`, so they survive `cargo clean`.

use std::{
    env, fs,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

use flate2::read::GzDecoder;
use tokio::io::AsyncWriteExt;

const FD_VERSION: &str = "10.5.0";
const RIPGREP_VERSION: &str = "15.2.0";
const JAQ_VERSION: &str = "3.1.1";
const JQ_VERSION: &str = "1.8.2";
const UV_VERSION: &str = "0.12.16";
const DENO_VERSION: &str = "2.9.7";

/// Size matters more than build time here: the payload is embedded in the server
/// and shipped to every host.
const ZSTD_LEVEL: i32 = 19;

/// The build needs the network, which can flake; fail fast rather than hang.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let target = env::var("TARGET").expect("cargo sets TARGET");
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    let cache = cache_dir();

    let sources = sources(&target);
    download_all(&sources, &cache);

    let mut manifest = Manifest::new();
    for source in &sources {
        source.embed(&mut manifest, &out_dir, &cache);
    }
    manifest.write(&out_dir);
}

/// `fd` and `rg` are the core tools and enter every build; the rest follow the
/// features that were enabled.
fn sources(target: &str) -> Vec<Source> {
    let mut sources = vec![
        Source::archive(
            format!("https://github.com/sharkdp/fd/releases/download/v{FD_VERSION}"),
            format!("fd-v{FD_VERSION}-{target}.tar.gz"),
            &["fd"],
        ),
        Source::archive(
            format!("https://github.com/BurntSushi/ripgrep/releases/download/{RIPGREP_VERSION}"),
            format!("ripgrep-{RIPGREP_VERSION}-{target}.tar.gz"),
            &["rg"],
        ),
    ];

    if env::var_os("CARGO_FEATURE_JAQ").is_some() {
        sources.push(Source::binary(
            format!("https://github.com/01mf02/jaq/releases/download/v{JAQ_VERSION}"),
            format!("jaq-{target}"),
            "jaq",
        ));
    }

    if env::var_os("CARGO_FEATURE_JQ").is_some() {
        sources.push(Source::binary(
            format!("https://github.com/jqlang/jq/releases/download/jq-{JQ_VERSION}"),
            jq_asset(target).to_owned(),
            "jq",
        ));
    }

    if env::var_os("CARGO_FEATURE_UV").is_some() {
        sources.push(Source::archive(
            format!("https://github.com/astral-sh/uv/releases/download/{UV_VERSION}"),
            format!("uv-{target}.tar.gz"),
            &["uv", "uvx"],
        ));
    }

    if env::var_os("CARGO_FEATURE_DENO").is_some() {
        sources.push(Source::archive(
            format!("https://github.com/denoland/deno/releases/download/v{DENO_VERSION}"),
            format!("deno-{target}.zip"),
            &["deno"],
        ));
    }

    sources
}

/// `jq` names its assets by OS and architecture instead of the Rust target triple,
/// so the triple is mapped explicitly; an unmatched target fails the build.
fn jq_asset(target: &str) -> &'static str {
    match target {
        "aarch64-apple-darwin" => "jq-macos-arm64",
        "x86_64-apple-darwin" => "jq-macos-amd64",
        "aarch64-unknown-linux-gnu" | "aarch64-unknown-linux-musl" => "jq-linux-arm64",
        "x86_64-unknown-linux-gnu" | "x86_64-unknown-linux-musl" => "jq-linux-amd64",
        other => panic!("jq has no prebuilt asset for target `{other}`"),
    }
}

/// One upstream release asset and the executables embedded out of it.
struct Source {
    url: String,
    asset: String,
    kind: Kind,
}

enum Kind {
    /// A `tar.gz` or `zip` holding the named executables.
    Archive(&'static [&'static str]),
    /// A bare executable stored under its own name.
    Binary(&'static str),
}

impl Source {
    fn archive(base_url: String, asset: String, members: &'static [&'static str]) -> Self {
        let url = format!("{base_url}/{asset}");
        Self {
            url,
            asset,
            kind: Kind::Archive(members),
        }
    }

    fn binary(base_url: String, asset: String, name: &'static str) -> Self {
        let url = format!("{base_url}/{asset}");
        Self {
            url,
            asset,
            kind: Kind::Binary(name),
        }
    }

    /// The tool names this asset yields, in the order they enter the manifest.
    fn names(&self) -> &[&'static str] {
        match &self.kind {
            Kind::Archive(members) => members,
            Kind::Binary(name) => std::slice::from_ref(name),
        }
    }

    fn embed(&self, manifest: &mut Manifest, out_dir: &Path, cache: &Path) {
        let path = cache.join(&self.asset);
        for name in self.names() {
            let bytes = match &self.kind {
                Kind::Archive(_) => extract_member(&path, name),
                Kind::Binary(_) => fs::read(&path)
                    .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display())),
            };
            let digest = write_zst(&out_dir.join(format!("{name}.zst")), &bytes);
            manifest.add(name, digest);
        }
    }
}

/// Fetch every missing asset into the cache, in parallel, before any extraction.
fn download_all(sources: &[Source], cache: &Path) {
    fs::create_dir_all(cache)
        .unwrap_or_else(|error| panic!("failed to create {}: {error}", cache.display()));

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("building the async runtime failed");

    let failures = runtime.block_on(async {
        let client = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .retry(retry_policy())
            .build()
            .expect("the HTTP client always builds");

        let mut tasks = tokio::task::JoinSet::new();
        for source in sources {
            let destination = cache.join(&source.asset);
            if destination.is_file() {
                continue;
            }

            let client = client.clone();
            let url = source.url.clone();
            let asset = source.asset.clone();
            tasks.spawn(async move { download(&client, &url, &asset, &destination).await });
        }

        let mut failures = Vec::new();
        while let Some(joined) = tasks.join_next().await {
            match joined {
                Ok(Ok(())) => {}
                Ok(Err(error)) => failures.push(error),
                Err(error) => failures.push(format!("a download task failed: {error}")),
            }
        }
        failures
    });

    if !failures.is_empty() {
        panic!(
            "failed to fetch the release assets:\n  {}",
            failures.join("\n  ")
        );
    }
}

/// Retry through reqwest's own policy: transient transport errors and 5xx are
/// retried, while a 4xx (for example a missing asset) fails immediately.
fn retry_policy() -> reqwest::retry::Builder {
    reqwest::retry::for_host(AnyHost)
        .max_retries_per_request(3)
        .classify_fn(|attempt| match attempt.status() {
            Some(status) if status.is_server_error() => attempt.retryable(),
            Some(_) => attempt.success(),
            None => attempt.retryable(),
        })
}

/// GitHub redirects asset downloads away from `github.com`, and reqwest scopes a
/// retry to the request host; match every host so redirected requests retry too.
struct AnyHost;

impl PartialEq<&str> for AnyHost {
    fn eq(&self, _: &&str) -> bool {
        true
    }
}

/// Download through a process-unique staging file and rename it into the cache, so
/// a failed or concurrent build never leaves a partial asset to be reused.
async fn download(
    client: &reqwest::Client,
    url: &str,
    asset: &str,
    destination: &Path,
) -> Result<(), String> {
    let staging = destination.with_file_name(format!("{asset}.{}.part", std::process::id()));

    match stream_to_file(client, url, &staging).await {
        Ok(()) => tokio::fs::rename(&staging, destination)
            .await
            .map_err(|error| format!("{url}: failed to finalize the download: {error}")),
        Err(error) => {
            let _ = tokio::fs::remove_file(&staging).await;
            Err(format!("{url}: {error}"))
        }
    }
}

/// Stream the response body to a file; the client's retry policy handles
/// transient failures underneath.
async fn stream_to_file(
    client: &reqwest::Client,
    url: &str,
    destination: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut response = client.get(url).send().await?.error_for_status()?;
    let mut file = tokio::fs::File::create(destination).await?;
    while let Some(chunk) = response.chunk().await? {
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    Ok(())
}

/// The digest of every embedded payload, keyed by tool name. The runtime pairs a
/// materialized file with its entry and rewrites it only when they differ.
struct Manifest {
    entries: Vec<(&'static str, [u8; 32])>,
}

impl Manifest {
    fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    fn add(&mut self, name: &'static str, digest: blake3::Hash) {
        self.entries.push((name, *digest.as_bytes()));
    }

    fn write(&self, out_dir: &Path) {
        let mut source = String::from("pub(crate) const ENTRIES: &[(&str, [u8; 32])] = &[\n");
        for (name, digest) in &self.entries {
            source.push_str(&format!("    (\"{name}\", ["));
            for byte in digest {
                source.push_str(&format!("0x{byte:02x}, "));
            }
            source.push_str("]),\n");
        }
        source.push_str("];\n");

        let dest = out_dir.join("manifest.rs");
        fs::write(&dest, source)
            .unwrap_or_else(|error| panic!("failed to write {}: {error}", dest.display()));
    }
}

fn extract_member(archive: &Path, name: &str) -> Vec<u8> {
    let file = fs::File::open(archive)
        .unwrap_or_else(|error| panic!("failed to open {}: {error}", archive.display()));
    let extension = archive
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();

    match extension {
        "zip" => extract_from_zip(file, archive, name),
        _ => extract_from_tar_gz(file, archive, name),
    }
}

fn extract_from_zip(file: fs::File, archive: &Path, name: &str) -> Vec<u8> {
    let mut zip = zip::ZipArchive::new(file)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", archive.display()));

    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", archive.display()));
        if entry.is_dir() {
            continue;
        }
        let file_name = entry
            .name()
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_owned();
        if file_name != name {
            continue;
        }
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).unwrap_or_else(|error| {
            panic!(
                "failed to read `{name}` from {}: {error}",
                archive.display()
            )
        });
        return bytes;
    }

    panic!("{} does not contain `{name}`", archive.display());
}

fn extract_from_tar_gz(file: fs::File, archive: &Path, name: &str) -> Vec<u8> {
    let mut tar = tar::Archive::new(GzDecoder::new(file));
    let entries = tar
        .entries()
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", archive.display()));

    for entry in entries {
        let mut entry =
            entry.unwrap_or_else(|error| panic!("failed to read {}: {error}", archive.display()));
        let path = entry
            .path()
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", archive.display()));
        let file_name = path
            .file_name()
            .and_then(|file_name| file_name.to_str())
            .unwrap_or_default()
            .to_owned();
        if file_name != name {
            continue;
        }
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).unwrap_or_else(|error| {
            panic!(
                "failed to read `{name}` from {}: {error}",
                archive.display()
            )
        });
        return bytes;
    }

    panic!("{} does not contain `{name}`", archive.display());
}

fn write_zst(dest: &Path, bytes: &[u8]) -> blake3::Hash {
    let digest = blake3::hash(bytes);
    let compressed = zstd::stream::encode_all(bytes, ZSTD_LEVEL)
        .unwrap_or_else(|error| panic!("failed to compress {}: {error}", dest.display()));
    fs::write(dest, compressed)
        .unwrap_or_else(|error| panic!("failed to write {}: {error}", dest.display()));
    digest
}

fn cache_dir() -> PathBuf {
    if let Some(path) = env::var_os("SANDBOX_TOOLKIT_CACHE") {
        return PathBuf::from(path);
    }

    env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(env::temp_dir)
        .join("sandbox-toolkit-binaries")
}
