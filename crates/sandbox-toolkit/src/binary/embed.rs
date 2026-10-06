use std::io;

pub struct Tool {
    pub name: &'static str,
    pub payload: &'static [u8],
    pub digest: &'static [u8; 32],
}

impl Tool {
    pub fn bytes(&self) -> io::Result<Vec<u8>> {
        zstd::stream::decode_all(self.payload)
    }
}

/// `uv`'s release ships `uvx` beside it, so six upstream releases cover the
/// seven executables the toolkit deploys.
pub static TOOLS: [Tool; 7] = [
    Tool {
        name: "fd",
        payload: include_bytes!(concat!(env!("OUT_DIR"), "/fd.zst")),
        digest: include_bytes!(concat!(env!("OUT_DIR"), "/fd.blake3")),
    },
    Tool {
        name: "rg",
        payload: include_bytes!(concat!(env!("OUT_DIR"), "/rg.zst")),
        digest: include_bytes!(concat!(env!("OUT_DIR"), "/rg.blake3")),
    },
    Tool {
        name: "curl",
        payload: include_bytes!(concat!(env!("OUT_DIR"), "/curl.zst")),
        digest: include_bytes!(concat!(env!("OUT_DIR"), "/curl.blake3")),
    },
    Tool {
        name: "uv",
        payload: include_bytes!(concat!(env!("OUT_DIR"), "/uv.zst")),
        digest: include_bytes!(concat!(env!("OUT_DIR"), "/uv.blake3")),
    },
    Tool {
        name: "uvx",
        payload: include_bytes!(concat!(env!("OUT_DIR"), "/uvx.zst")),
        digest: include_bytes!(concat!(env!("OUT_DIR"), "/uvx.blake3")),
    },
    Tool {
        name: "deno",
        payload: include_bytes!(concat!(env!("OUT_DIR"), "/deno.zst")),
        digest: include_bytes!(concat!(env!("OUT_DIR"), "/deno.blake3")),
    },
    Tool {
        name: "tusd",
        payload: include_bytes!(concat!(env!("OUT_DIR"), "/tusd.zst")),
        digest: include_bytes!(concat!(env!("OUT_DIR"), "/tusd.blake3")),
    },
];
