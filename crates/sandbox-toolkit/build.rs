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
const CURL: (&str, &str) = ("8.22.0", "curl");
const UV: (&str, &str) = ("0.12.16", "uv");
const DENO: (&str, &str) = ("2.9.7", "deno");
const TUSD: (&str, &str) = ("2.10.1", "tusd");
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let target = env::var("TARGET").expect("cargo sets TARGET");
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    let cache = out.join("cache");
    let sources = sources(&target);

    download_all(&sources, &cache);

    for source in &sources {
        source.embed(&out, &cache);
    }
}

struct Source {
    url: String,
    asset: String,
    names: &'static [&'static str],
}

impl Source {
    fn archive(base: String, asset: String, names: &'static [&'static str]) -> Self {
        Self {
            url: format!("{base}/{asset}"),
            asset,
            names,
        }
    }

    fn embed(&self, out: &Path, cache: &Path) {
        let archive = cache.join(&self.asset);
        for name in self.names.iter().copied() {
            let bytes = extract(&archive, name);
            let hash = blake3::hash(&bytes);
            // The embedded digest is taken over the raw bytes, so the level only
            // trades payload size against build time: 19 is ~12% smaller than 9 for
            // ~14x the time.
            let compressed = zstd::stream::encode_all(bytes.as_slice(), 9)
                .unwrap_or_else(|error| panic!("failed to compress {name}: {error}"));
            fs::write(out.join(format!("{name}.zst")), compressed)
                .unwrap_or_else(|error| panic!("failed to write {name}.zst: {error}"));
            // The digest is embedded next to the payload so the two cannot drift apart.
            fs::write(out.join(format!("{name}.blake3")), hash.as_bytes())
                .unwrap_or_else(|error| panic!("failed to write {name}.blake3: {error}"));
        }
    }
}

fn sources(target: &str) -> Vec<Source> {
    vec![
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
            rg_asset(target),
            &[RG.1],
        ),
        Source::archive(
            format!(
                "https://github.com/stunnel/static-curl/releases/download/{}",
                CURL.0
            ),
            curl_asset(target),
            &[CURL.1],
        ),
        Source::archive(
            format!("https://github.com/astral-sh/uv/releases/download/{}", UV.0),
            format!("uv-{target}.tar.gz"),
            &["uv", "uvx"],
        ),
        Source::archive(
            format!(
                "https://github.com/denoland/deno/releases/download/v{}",
                DENO.0
            ),
            format!("deno-{target}.zip"),
            &[DENO.1],
        ),
        Source::archive(
            format!("https://github.com/tus/tusd/releases/download/v{}", TUSD.0),
            tusd_asset(target),
            &[TUSD.1],
        ),
    ]
}

/// ripgrep builds x86_64 Linux only against musl, so that is the asset to embed
/// on a glibc target as well: it is statically linked and needs no libc.
fn rg_asset(target: &str) -> String {
    let release = match target {
        "x86_64-unknown-linux-gnu" => "x86_64-unknown-linux-musl",
        target => target,
    };

    format!("ripgrep-{}-{release}.tar.gz", RG.0)
}

/// static-curl names its assets by `uname -m` and offers a musl and a glibc
/// build; the musl one is statically linked and needs no libc, matching `rg`'s.
fn curl_asset(target: &str) -> String {
    let arch = match target {
        "x86_64-unknown-linux-gnu" => "x86_64",
        "aarch64-unknown-linux-gnu" => "aarch64",
        target => panic!("static curl is not embedded for {target}"),
    };

    format!("curl-linux-{arch}-musl-{}.tar.xz", CURL.0)
}

/// tusd is a Go program, so its release assets are named by `GOOS`/`GOARCH`
/// rather than by the target triple the other tools identify themselves with.
///
/// The asset is the one for the target the build script runs for, so a host
/// tusd is not released for fails here instead of fetching the wrong binary.
fn tusd_asset(target: &str) -> String {
    let os = match target {
        target if target.contains("linux") => "linux",
        target if target.contains("apple-darwin") => "darwin",
        target => panic!("tusd is not released for {target}"),
    };
    let arch = match target {
        target if target.starts_with("x86_64") => "amd64",
        target if target.starts_with("aarch64") => "arm64",
        target => panic!("tusd is not released for {target}"),
    };
    let extension = if os == "linux" { "tar.gz" } else { "zip" };

    format!("tusd_{os}_{arch}.{extension}")
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
    let bytes = match archive.extension().and_then(|x| x.to_str()) {
        Some("zip") => from_zip(
            zip::ZipArchive::new(file)
                .unwrap_or_else(|error| panic!("{}: {error}", archive.display())),
            name,
            archive,
        ),
        Some("xz") => from_tar(
            tar::Archive::new(liblzma::read::XzDecoder::new(file)),
            name,
            archive,
        ),
        _ => from_tar(tar::Archive::new(GzDecoder::new(file)), name, archive),
    };

    bytes.unwrap_or_else(|| panic!("{} does not contain `{name}`", archive.display()))
}

fn from_tar<R: Read>(mut tar: tar::Archive<R>, name: &str, archive: &Path) -> Option<Vec<u8>> {
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
            return Some(bytes);
        }
    }

    None
}

fn from_zip(mut zip: zip::ZipArchive<fs::File>, name: &str, archive: &Path) -> Option<Vec<u8>> {
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .unwrap_or_else(|error| panic!("{}: {error}", archive.display()));
        if !entry.is_dir() && entry.name().rsplit('/').next() == Some(name) {
            let mut bytes = Vec::new();
            entry
                .read_to_end(&mut bytes)
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            return Some(bytes);
        }
    }

    None
}
