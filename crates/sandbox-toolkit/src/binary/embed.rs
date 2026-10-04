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

/// `uv`'s release ships `uvx` beside it, so four upstream tools cover five
/// executables.
pub static TOOLS: [Tool; 5] = [
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
];
