use super::BinaryError;

/// The build script writes this next to the payloads; it pairs every embedded tool
/// with the blake3 digest of its decompressed bytes.
mod manifest {
    include!(concat!(env!("OUT_DIR"), "/manifest.rs"));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Binary {
    pub(super) name: &'static str,
    /// The zstd-compressed executable, as embedded by `build.rs`.
    pub(super) payload: &'static [u8],
}

impl Binary {
    /// The digest the build script recorded for the decompressed payload.
    pub(super) fn digest(self) -> Result<[u8; 32], BinaryError> {
        manifest::ENTRIES
            .iter()
            .find_map(|(name, digest)| (*name == self.name).then_some(*digest))
            .ok_or(BinaryError::MissingDigest { binary: self.name })
    }
}

/// `fd` — a simple, fast and user-friendly alternative to `find`.
pub(super) const FD: Binary = Binary {
    name: "fd",
    payload: include_bytes!(concat!(env!("OUT_DIR"), "/fd.zst")),
};

/// `rg` — ripgrep, a line-oriented recursive search tool that respects
/// `.gitignore` by default.
pub(super) const RIPGREP: Binary = Binary {
    name: "rg",
    payload: include_bytes!(concat!(env!("OUT_DIR"), "/rg.zst")),
};

/// `jaq` — "just another JSON query tool", a `jq` clone.
#[cfg(feature = "jaq")]
pub(super) const JAQ: Binary = Binary {
    name: "jaq",
    payload: include_bytes!(concat!(env!("OUT_DIR"), "/jaq.zst")),
};

/// `jq` — a lightweight and flexible command-line JSON processor.
#[cfg(feature = "jq")]
pub(super) const JQ: Binary = Binary {
    name: "jq",
    payload: include_bytes!(concat!(env!("OUT_DIR"), "/jq.zst")),
};

/// `uv` — an extremely fast Python package and project manager.
#[cfg(feature = "uv")]
pub(super) const UV: Binary = Binary {
    name: "uv",
    payload: include_bytes!(concat!(env!("OUT_DIR"), "/uv.zst")),
};

/// `uvx` — run a Python tool without installing it, `uv`'s `pipx` equivalent.
///
/// It execs the `uv` sitting next to it, so both must land in the same directory.
#[cfg(feature = "uv")]
pub(super) const UVX: Binary = Binary {
    name: "uvx",
    payload: include_bytes!(concat!(env!("OUT_DIR"), "/uvx.zst")),
};

/// `deno` — a secure JavaScript and TypeScript runtime.
#[cfg(feature = "deno")]
pub(super) const DENO: Binary = Binary {
    name: "deno",
    payload: include_bytes!(concat!(env!("OUT_DIR"), "/deno.zst")),
};

/// Every tool selected by the enabled features; `fd` and `rg` are always present.
pub(super) fn bundled() -> Vec<Binary> {
    #[allow(unused_mut)]
    let mut binaries = vec![FD, RIPGREP];

    #[cfg(feature = "jaq")]
    binaries.push(JAQ);

    #[cfg(feature = "jq")]
    binaries.push(JQ);

    #[cfg(feature = "uv")]
    {
        binaries.push(UV);
        binaries.push(UVX);
    }

    #[cfg(feature = "deno")]
    binaries.push(DENO);

    binaries
}
