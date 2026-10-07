use std::io::{self, Read as _};

pub struct Tool {
    pub name: &'static str,
    pub payload: &'static [u8],
    pub digest: &'static [u8; 32],
}

impl Tool {
    pub fn bytes(&self) -> io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        liblzma::read::XzDecoder::new(self.payload).read_to_end(&mut bytes)?;
        Ok(bytes)
    }
}

/// `uv`'s release ships `uvx` beside it, so six upstream releases cover the
/// seven executables the toolkit deploys.
pub static TOOLS: [Tool; 7] = [
    Tool {
        name: "fd",
        payload: include_bytes!(concat!(env!("OUT_DIR"), "/fd.xz")),
        digest: include_bytes!(concat!(env!("OUT_DIR"), "/fd.blake3")),
    },
    Tool {
        name: "rg",
        payload: include_bytes!(concat!(env!("OUT_DIR"), "/rg.xz")),
        digest: include_bytes!(concat!(env!("OUT_DIR"), "/rg.blake3")),
    },
    Tool {
        name: "curl",
        payload: include_bytes!(concat!(env!("OUT_DIR"), "/curl.xz")),
        digest: include_bytes!(concat!(env!("OUT_DIR"), "/curl.blake3")),
    },
    Tool {
        name: "uv",
        payload: include_bytes!(concat!(env!("OUT_DIR"), "/uv.xz")),
        digest: include_bytes!(concat!(env!("OUT_DIR"), "/uv.blake3")),
    },
    Tool {
        name: "uvx",
        payload: include_bytes!(concat!(env!("OUT_DIR"), "/uvx.xz")),
        digest: include_bytes!(concat!(env!("OUT_DIR"), "/uvx.blake3")),
    },
    Tool {
        name: "deno",
        payload: include_bytes!(concat!(env!("OUT_DIR"), "/deno.xz")),
        digest: include_bytes!(concat!(env!("OUT_DIR"), "/deno.blake3")),
    },
    Tool {
        name: "tusd",
        payload: include_bytes!(concat!(env!("OUT_DIR"), "/tusd.xz")),
        digest: include_bytes!(concat!(env!("OUT_DIR"), "/tusd.blake3")),
    },
];
