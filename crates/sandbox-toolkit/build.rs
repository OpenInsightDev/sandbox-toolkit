use std::{
    env, fs,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

use flate2::read::GzDecoder;
use tokio::io::AsyncWriteExt;

const FD: (&str, &str) = ("10.5.0", "fd");
const RG: (&str, &str) = ("15.2.0", "rg");
const JAQ: (&str, &str) = ("3.1.1", "jaq");
const JQ: (&str, &str) = ("1.8.2", "jq");
const UV: (&str, &str) = ("0.12.16", "uv");
const DENO: (&str, &str) = ("2.9.7", "deno");
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let target = env::var("TARGET").expect("cargo sets TARGET");
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    let cache = out.join("cache");
    let sources = sources(&target);

    download_all(&sources, &cache);

    let entries = sources
        .iter()
        .flat_map(|source| source.embed(&out, &cache))
        .collect::<Vec<_>>();
    let manifest = entries
        .iter()
        .map(|(name, hash)| {
            format!(
                "    (\"{name}\", [{}]),\n",
                hash.iter()
                    .map(|byte| format!("0x{byte:02x}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
        .collect::<String>();
    fs::write(
        out.join("manifest.rs"),
        format!("pub(crate) const ENTRIES: &[(&str, [u8; 32])] = &[\n{manifest}];\n"),
    )
    .expect("failed to write manifest.rs");
}

struct Source {
    url: String,
    asset: String,
    names: &'static [&'static str],
    binary: Option<&'static str>,
}

impl Source {
    fn archive(base: String, asset: String, names: &'static [&'static str]) -> Self {
        Self {
            url: format!("{base}/{asset}"),
            asset,
            names,
            binary: None,
        }
    }

    fn binary(base: String, asset: String, name: &'static str) -> Self {
        Self {
            url: format!("{base}/{asset}"),
            asset,
            names: &[],
            binary: Some(name),
        }
    }

    fn embed(&self, out: &Path, cache: &Path) -> Vec<(&'static str, [u8; 32])> {
        let archive = cache.join(&self.asset);
        self.names
            .iter()
            .copied()
            .chain(self.binary)
            .map(|name| {
                let bytes = if self.binary.is_some() {
                    fs::read(&archive)
                        .unwrap_or_else(|error| panic!("{}: {error}", archive.display()))
                } else {
                    extract(&archive, name)
                };
                let hash = blake3::hash(&bytes);
                let compressed = zstd::stream::encode_all(bytes.as_slice(), 19)
                    .unwrap_or_else(|error| panic!("failed to compress {name}: {error}"));
                fs::write(out.join(format!("{name}.zst")), compressed)
                    .unwrap_or_else(|error| panic!("failed to write {name}.zst: {error}"));
                (name, *hash.as_bytes())
            })
            .collect()
    }
}

fn sources(target: &str) -> Vec<Source> {
    let mut sources = vec![
        Source::archive(
            format!("https://github.com/sharkdp/fd/releases/download/v{}", FD.0),
            format!("fd-v{}-{target}.tar.gz", FD.0),
            &[FD.1],
        ),
        Source::archive(
            format!(
                "https://github.com/BurntSushi/ripgrep/releases/download/{}",
                RG.0
            ),
            format!("ripgrep-{}-{target}.tar.gz", RG.0),
            &[RG.1],
        ),
    ];

    if env::var_os("CARGO_FEATURE_JAQ").is_some() {
        sources.push(Source::binary(
            format!("https://github.com/01mf02/jaq/releases/download/v{}", JAQ.0),
            format!("jaq-{target}"),
            JAQ.1,
        ));
    }
    if env::var_os("CARGO_FEATURE_JQ").is_some() {
        sources.push(Source::binary(
            format!("https://github.com/jqlang/jq/releases/download/jq-{}", JQ.0),
            jq_asset(target).into(),
            JQ.1,
        ));
    }
    if env::var_os("CARGO_FEATURE_UV").is_some() {
        sources.push(Source::archive(
            format!("https://github.com/astral-sh/uv/releases/download/{}", UV.0),
            format!("uv-{target}.tar.gz"),
            &["uv", "uvx"],
        ));
    }
    if env::var_os("CARGO_FEATURE_DENO").is_some() {
        sources.push(Source::archive(
            format!(
                "https://github.com/denoland/deno/releases/download/v{}",
                DENO.0
            ),
            format!("deno-{target}.zip"),
            &[DENO.1],
        ));
    }
    sources
}

fn jq_asset(target: &str) -> &'static str {
    match target {
        "aarch64-apple-darwin" => "jq-macos-arm64",
        "x86_64-apple-darwin" => "jq-macos-amd64",
        "aarch64-unknown-linux-gnu" | "aarch64-unknown-linux-musl" => "jq-linux-arm64",
        "x86_64-unknown-linux-gnu" | "x86_64-unknown-linux-musl" => "jq-linux-amd64",
        _ => panic!("jq has no prebuilt asset for target `{target}`"),
    }
}

fn download_all(sources: &[Source], cache: &Path) {
    fs::create_dir_all(cache).unwrap_or_else(|error| panic!("{}: {error}", cache.display()));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to build runtime");

    let failures = runtime.block_on(async {
        let client = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .expect("failed to build HTTP client");
        let mut tasks = tokio::task::JoinSet::new();

        for source in sources {
            let destination = cache.join(&source.asset);
            if destination.is_file() {
                continue;
            }
            let client = client.clone();
            let url = source.url.clone();
            tasks.spawn(async move { download(&client, &url, &destination).await });
        }

        let mut failures = Vec::new();
        while let Some(result) = tasks.join_next().await {
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => failures.push(error),
                Err(error) => failures.push(error.to_string()),
            }
        }
        failures
    });
    if !failures.is_empty() {
        panic!(
            "failed to fetch release assets:\n  {}",
            failures.join("\n  ")
        );
    }
}

async fn download(client: &reqwest::Client, url: &str, destination: &Path) -> Result<(), String> {
    let staging = destination.with_file_name(format!(
        "{}.{}.part",
        destination.file_name().unwrap().to_string_lossy(),
        std::process::id()
    ));
    let result = async {
        let mut response = client.get(url).send().await?.error_for_status()?;
        let mut file = tokio::fs::File::create(&staging).await?;
        while let Some(chunk) = response.chunk().await? {
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        tokio::fs::rename(&staging, destination).await?;
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&staging).await;
    }
    result.map_err(|error| format!("{url}: {error}"))
}

fn extract(archive: &Path, name: &str) -> Vec<u8> {
    let file =
        fs::File::open(archive).unwrap_or_else(|error| panic!("{}: {error}", archive.display()));
    if archive.extension().and_then(|x| x.to_str()) == Some("zip") {
        let mut zip = zip::ZipArchive::new(file)
            .unwrap_or_else(|error| panic!("{}: {error}", archive.display()));
        for index in 0..zip.len() {
            let mut entry = zip
                .by_index(index)
                .unwrap_or_else(|error| panic!("{}: {error}", archive.display()));
            if !entry.is_dir() && entry.name().rsplit('/').next() == Some(name) {
                let mut bytes = Vec::new();
                entry
                    .read_to_end(&mut bytes)
                    .unwrap_or_else(|error| panic!("{name}: {error}"));
                return bytes;
            }
        }
    } else {
        let mut tar = tar::Archive::new(GzDecoder::new(file));
        for entry in tar
            .entries()
            .unwrap_or_else(|error| panic!("{}: {error}", archive.display()))
        {
            let mut entry = entry.unwrap_or_else(|error| panic!("{}: {error}", archive.display()));
            let matches = entry
                .path()
                .ok()
                .map(|path| path.file_name().and_then(|name| name.to_str()) == Some(name))
                .unwrap_or(false);
            if matches {
                let mut bytes = Vec::new();
                entry
                    .read_to_end(&mut bytes)
                    .unwrap_or_else(|error| panic!("{name}: {error}"));
                return bytes;
            }
        }
    }
    panic!("{} does not contain `{name}`", archive.display());
}
