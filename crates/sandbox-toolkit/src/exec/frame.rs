use bytes::{BufMut, Bytes, BytesMut};

use crate::exec::model::ExecStatus;
use crate::exec::runtime::Event;

/// The media type of an upgraded response, which carries frames rather than
/// JSON.
pub const CONTENT_TYPE: &str = "application/vnd.sandbox-toolkit.exec-stream";

/// stdin is reserved on the same numbering; exec never uses it.
const STDOUT: u8 = 1;
const STDERR: u8 = 2;
const ERROR: u8 = 3;

/// One channel byte and a four-byte big-endian payload length.
const HEADER_LEN: usize = 5;

/// The most one frame carries; a larger payload is split across frames.
const MAX_PAYLOAD_LEN: usize = 4 * 1024 * 1024;

/// Encodes an event as the frames that carry it.
pub fn encode(event: Event) -> Bytes {
    let (channel, payload) = match event {
        Event::Stdout(bytes) => (STDOUT, bytes),
        Event::Stderr(bytes) => (STDERR, bytes),
        Event::Status(status) => (ERROR, status_bytes(status)),
    };

    let mut frames = BytesMut::with_capacity(payload.len() + HEADER_LEN);
    for chunk in payload.chunks(MAX_PAYLOAD_LEN) {
        frames.put_u8(channel);
        frames.put_u32(chunk.len() as u32);
        frames.put_slice(chunk);
    }

    frames.freeze()
}

fn status_bytes(status: ExecStatus) -> Bytes {
    Bytes::from(serde_json::to_vec(&status).expect("the terminal status encodes to JSON"))
}
