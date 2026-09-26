//! The command-line tools shipped with the server.
//!
//! `build.rs` embeds a zstd payload for every tool the enabled features select and
//! records the blake3 digest of each tool's decompressed bytes. At startup
//! [`materialize`] writes the tools into [`materialized_dir`], skipping any file
//! that already matches its digest, so sandboxed children can exec them by bare
//! name without a host install or a network fetch.

mod catalog;
mod materialize;

use std::path::PathBuf;

use thiserror::Error;

pub(crate) use self::materialize::{materialize, materialized_dir};

#[derive(Debug, Error)]
pub(crate) enum BinaryError {
    #[error(
        "the `HOME` environment variable is not set, so the binaries directory cannot be resolved"
    )]
    MissingHome,

    #[error("the embedded manifest has no digest for binary `{binary}`")]
    MissingDigest { binary: &'static str },

    #[error("failed to create the binaries directory {}", dir.display())]
    CreateDir {
        dir: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to read the existing binary `{binary}` at {}", path.display())]
    Read {
        binary: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("the embedded payload for `{binary}` does not match its manifest digest")]
    Corrupt { binary: &'static str },

    #[error("failed to materialize binary `{binary}` to {}", destination.display())]
    Materialize {
        binary: &'static str,
        destination: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("binary materialization task failed")]
    Task(#[from] tokio::task::JoinError),
}

#[cfg(test)]
mod tests {
    use std::{
        os::unix::fs::PermissionsExt,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::{BinaryError, catalog, materialize, materialized_dir};

    fn decompressed(binary: catalog::Binary) -> Vec<u8> {
        zstd::stream::decode_all(binary.payload).expect("inflating the payload failed")
    }

    fn digest_of(bytes: &[u8]) -> [u8; 32] {
        *blake3::hash(bytes).as_bytes()
    }

    /// A directory unique to one test, so the tests never share materialized files.
    fn scratch_dir() -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let sequence = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "sandbox-toolkit-test-{}-{sequence}",
            std::process::id()
        ))
    }

    #[test]
    fn every_payload_matches_its_manifest_digest() {
        for binary in catalog::bundled() {
            let bytes = decompressed(binary);
            assert!(!bytes.is_empty(), "{} was embedded empty", binary.name);
            assert_eq!(
                digest_of(&bytes),
                binary.digest().expect("the manifest is missing an entry"),
                "{} does not match its manifest digest",
                binary.name
            );
        }
    }

    #[test]
    fn materialized_dir_is_under_home() {
        assert_eq!(
            super::materialize::dir_under(Some("/home/example".into())).unwrap(),
            PathBuf::from("/home/example/.sandbox-toolkit/bin")
        );
        assert!(matches!(
            super::materialize::dir_under(None),
            Err(BinaryError::MissingHome)
        ));
        assert!(matches!(
            super::materialize::dir_under(Some("".into())),
            Err(BinaryError::MissingHome)
        ));
    }

    #[tokio::test]
    async fn materialize_writes_and_repairs_bundled_binaries() {
        let dir = scratch_dir();
        super::materialize::materialize_into(&dir)
            .await
            .expect("materializing binaries failed");

        for binary in catalog::bundled() {
            let path = dir.join(binary.name);
            let metadata = std::fs::metadata(&path)
                .unwrap_or_else(|error| panic!("{} was not materialized: {error}", path.display()));
            assert!(metadata.is_file());
            assert_eq!(
                metadata.permissions().mode() & 0o777,
                0o755,
                "{} is not executable",
                binary.name
            );
            assert_eq!(
                digest_of(&std::fs::read(&path).unwrap()),
                binary.digest().expect("the manifest is missing an entry"),
                "{} does not match its embedded payload",
                binary.name
            );
        }

        let fd = dir.join(catalog::FD.name);
        std::fs::write(&fd, b"tampered").expect("writing the tampered binary failed");
        super::materialize::materialize_into(&dir)
            .await
            .expect("rematerializing failed");
        assert_eq!(
            digest_of(&std::fs::read(&fd).unwrap()),
            catalog::FD
                .digest()
                .expect("the manifest is missing an entry"),
            "the tampered binary was not restored"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn embedded_binaries_are_runnable() {
        let dir = scratch_dir();
        super::materialize::materialize_into(&dir)
            .await
            .expect("materializing binaries failed");

        for binary in catalog::bundled() {
            let output = std::process::Command::new(dir.join(binary.name))
                .arg("--version")
                .output()
                .unwrap_or_else(|error| panic!("failed to run {}: {error}", binary.name));

            assert!(
                output.status.success(),
                "{} --version exited with {}: {}",
                binary.name,
                output.status,
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn materialize_uses_the_home_directory() {
        let dir = materialize().await.expect("materializing binaries failed");

        assert_eq!(dir, materialized_dir().expect("HOME is set in tests"));
        for binary in catalog::bundled() {
            assert!(
                dir.join(binary.name).is_file(),
                "{} was not materialized",
                binary.name
            );
        }
    }
}
