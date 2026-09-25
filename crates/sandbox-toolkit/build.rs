//! Downloads are cached outside `target/`, so they survive `cargo clean`.

use std::{
    env, fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::Command,
};

use flate2::{Compression, read::GzDecoder, write::GzEncoder};

const FD_VERSION: &str = "10.5.0";
const RIPGREP_VERSION: &str = "15.2.0";
const JAQ_VERSION: &str = "3.1.1";
const UV_VERSION: &str = "0.12.16";
const DENO_VERSION: &str = "2.9.7";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let target = env::var("TARGET").expect("cargo sets TARGET");
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    let cache = cache_dir();

    embed_archive(
        &out_dir,
        &cache,
        &format!("https://github.com/sharkdp/fd/releases/download/v{FD_VERSION}"),
        &format!("fd-v{FD_VERSION}-{target}.tar.gz"),
        &["fd"],
    );
    embed_archive(
        &out_dir,
        &cache,
        &format!("https://github.com/BurntSushi/ripgrep/releases/download/{RIPGREP_VERSION}"),
        &format!("ripgrep-{RIPGREP_VERSION}-{target}.tar.gz"),
        &["rg"],
    );

    if env::var_os("CARGO_FEATURE_JAQ").is_some() {
        embed_binary(
            &out_dir,
            &cache,
            &format!("https://github.com/01mf02/jaq/releases/download/v{JAQ_VERSION}"),
            &format!("jaq-{target}"),
            "jaq",
        );
    }

    if env::var_os("CARGO_FEATURE_UV").is_some() {
        embed_archive(
            &out_dir,
            &cache,
            &format!("https://github.com/astral-sh/uv/releases/download/{UV_VERSION}"),
            &format!("uv-{target}.tar.gz"),
            &["uv", "uvx"],
        );
    }

    if env::var_os("CARGO_FEATURE_DENO").is_some() {
        embed_archive(
            &out_dir,
            &cache,
            &format!("https://github.com/denoland/deno/releases/download/v{DENO_VERSION}"),
            &format!("deno-{target}.zip"),
            &["deno"],
        );
    }
}

fn embed_archive(out_dir: &Path, cache: &Path, base_url: &str, asset: &str, members: &[&str]) {
    let archive = fetch(&format!("{base_url}/{asset}"), asset, cache);
    for member in members {
        let bytes = extract_member(&archive, member);
        write_gz(&out_dir.join(format!("{member}.gz")), &bytes)
            .unwrap_or_else(|error| panic!("failed to gzip `{member}`: {error}"));
    }
}

fn embed_binary(out_dir: &Path, cache: &Path, base_url: &str, asset: &str, name: &str) {
    let path = fetch(&format!("{base_url}/{asset}"), asset, cache);
    let bytes = fs::read(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    write_gz(&out_dir.join(format!("{name}.gz")), &bytes)
        .unwrap_or_else(|error| panic!("failed to gzip `{name}`: {error}"));
}

fn fetch(url: &str, asset: &str, cache: &Path) -> PathBuf {
    let path = cache.join(asset);
    if path.is_file() {
        return path;
    }

    fs::create_dir_all(cache)
        .unwrap_or_else(|error| panic!("failed to create {}: {error}", cache.display()));

    let staging = cache.join(format!("{asset}.part"));
    let status = Command::new("curl")
        .args([
            "--fail",
            "--location",
            "--silent",
            "--show-error",
            "--retry",
            "3",
            "--output",
        ])
        .arg(&staging)
        .arg(url)
        .status();

    match status {
        Ok(status) if status.success() => {}
        Ok(status) => panic!("curl exited with {status} while downloading {url}"),
        Err(error) => panic!(
            "failed to run `curl` while downloading {url}: {error}\n\
             the build needs network access to fetch prebuilt binaries"
        ),
    }

    fs::rename(&staging, &path)
        .unwrap_or_else(|error| panic!("failed to finalize {}: {error}", path.display()));
    path
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

fn write_gz(dest: &Path, bytes: &[u8]) -> io::Result<()> {
    let file = fs::File::create(dest)?;
    let mut encoder = GzEncoder::new(file, Compression::best());
    encoder.write_all(bytes)?;
    encoder.finish()?;
    Ok(())
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
