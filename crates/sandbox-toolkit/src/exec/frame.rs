use bytes::{BufMut, Bytes, BytesMut};
use schemars::JsonSchema;
use serde::Serialize;
use ts_rs::TS;

use crate::exec::runtime::Event;

pub const CONTENT_TYPE: &str = "application/vnd.sandbox-toolkit.exec-stream";

// The channel numbering exec's frame stream and pty's messages share: 0 stdin,
// 1 stdout, 2 stderr, 3 error, 4 resize.
pub const STDIN: u8 = 0;
pub const STDOUT: u8 = 1;
pub const STDERR: u8 = 2;
pub const ERROR: u8 = 3;
pub const RESIZE: u8 = 4;

/// The most bytes one payload carries; longer output is split, and a payload
/// beyond it is a protocol error.
pub const MAX_PAYLOAD_LEN: usize = 4 * 1024 * 1024;

/// One channel byte and a four-byte big-endian payload length.
const HEADER_LEN: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema, TS)]
#[serde(tag = "status", rename_all = "snake_case")]
#[ts(export)]
pub enum Status {
    Exited { exit_code: i32 },
    Signaled { signal: i32 },
}

impl Status {
    pub fn payload(&self) -> Bytes {
        Bytes::from(serde_json::to_vec(self).expect("the terminal status encodes to JSON"))
    }
}

pub fn encode(event: Event) -> Bytes {
    let (channel, payload) = match event {
        Event::Stdout(bytes) => (STDOUT, bytes),
        Event::Stderr(bytes) => (STDERR, bytes),
        Event::Status(status) => (ERROR, status.payload()),
    };

    let mut frames = BytesMut::with_capacity(payload.len() + HEADER_LEN);
    for chunk in payload.chunks(MAX_PAYLOAD_LEN) {
        frames.put_u8(channel);
        frames.put_u32(chunk.len() as u32);
        frames.put_slice(chunk);
    }

    frames.freeze()
}
