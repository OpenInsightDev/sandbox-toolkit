//! Frames of the exec and shell response stream.
//!
//! Frames are length-prefixed rather than separated, because command output may
//! contain any byte a separator would use.
//!
//! Every stream ends with exactly one terminal [`Frame::Status`]; without it a
//! stream was truncated, not successful. [`FrameStream`] enforces that.
//!
//! This is not the pty format: a pty session runs over a WebSocket, where each
//! message is already a frame, so pty uses [`super::pty`] instead.

#![expect(
    dead_code,
    reason = "consumed by the exec and shell handlers, which are not written yet"
)]

use bytes::{Buf, BufMut, Bytes, BytesMut};

use super::model::Status;

/// A frame header: one channel byte and a four-byte big-endian length.
pub(crate) const HEADER_LEN: usize = 5;

/// The largest payload a frame may carry.
///
/// The peer controls the advertised length, so [`Frame::decode`] rejects anything
/// larger before allocating.
pub(crate) const MAX_PAYLOAD_LEN: usize = 4 * 1024 * 1024;

/// A multiplexed channel.
///
/// The discriminants are wire identifiers; reordering them changes the format.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Channel {
    /// Client to server: input for the remote process.
    Stdin = 0,
    /// Server to client: output of the remote process.
    Stdout = 1,
    /// Server to client: diagnostics of the remote process.
    Stderr = 2,
    /// Server to client: the terminal status, carrying success as well as failure.
    Error = 3,
    /// Client to server: a new terminal size.
    Resize = 4,
}

impl Channel {
    pub(crate) const fn as_u8(self) -> u8 {
        self as u8
    }

    pub(crate) fn from_u8(id: u8) -> Result<Self, FrameError> {
        match id {
            0 => Ok(Self::Stdin),
            1 => Ok(Self::Stdout),
            2 => Ok(Self::Stderr),
            3 => Ok(Self::Error),
            4 => Ok(Self::Resize),
            other => Err(FrameError::UnknownChannel(other)),
        }
    }
}

/// Terminal geometry in character cells.
///
/// Shared with the pty frames, so both encode the size the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TerminalSize {
    pub(crate) rows: u16,
    pub(crate) cols: u16,
}

impl TerminalSize {
    /// Bytes on the wire: rows then cols, both big-endian.
    pub(crate) const WIRE_LEN: usize = 4;

    pub(crate) fn to_wire(self) -> [u8; Self::WIRE_LEN] {
        let mut wire = [0u8; Self::WIRE_LEN];
        wire[..2].copy_from_slice(&self.rows.to_be_bytes());
        wire[2..].copy_from_slice(&self.cols.to_be_bytes());
        wire
    }

    /// Reads the leading [`Self::WIRE_LEN`] bytes and ignores any trailing bytes, so
    /// a later, larger encoding of the size stays decodable.
    pub(crate) fn from_wire(payload: &[u8]) -> Option<Self> {
        let [rows_hi, rows_lo, cols_hi, cols_lo, ..] = payload else {
            return None;
        };

        Some(Self {
            rows: u16::from_be_bytes([*rows_hi, *rows_lo]),
            cols: u16::from_be_bytes([*cols_hi, *cols_lo]),
        })
    }
}

/// One message on a multiplexed stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Frame {
    Stdin(Bytes),
    Stdout(Bytes),
    Stderr(Bytes),
    /// A resize request from the client.
    Resize(TerminalSize),
    /// The terminal status frame, on the error channel. Exactly one ends every
    /// stream, whether the command succeeded or failed.
    Status(Status),
}

impl Frame {
    pub(crate) const fn channel(&self) -> Channel {
        match self {
            Self::Stdin(_) => Channel::Stdin,
            Self::Stdout(_) => Channel::Stdout,
            Self::Stderr(_) => Channel::Stderr,
            Self::Resize(_) => Channel::Resize,
            Self::Status(_) => Channel::Error,
        }
    }

    /// Encodes the frame. Splitting payloads above [`MAX_PAYLOAD_LEN`] is the
    /// producer's responsibility, since only the decoder enforces that bound.
    pub(crate) fn encode(&self) -> Bytes {
        let payload = self.payload();
        let mut buffer = BytesMut::with_capacity(HEADER_LEN + payload.len());
        buffer.put_u8(self.channel().as_u8());
        buffer.put_u32(payload.len() as u32);
        buffer.put_slice(&payload);
        buffer.freeze()
    }

    /// Removes the first frame from `buffer`, or returns `None` while `buffer`
    /// holds less than a complete frame, leaving it untouched for a later retry.
    ///
    /// This is the raw reader; it does not track the terminal [`Frame::Status`].
    pub(crate) fn decode(buffer: &mut BytesMut) -> Result<Option<Self>, FrameError> {
        let Some(header) = buffer.get(..HEADER_LEN) else {
            return Ok(None);
        };

        let channel = Channel::from_u8(header[0])?;
        let length = u32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize;
        if length > MAX_PAYLOAD_LEN {
            return Err(FrameError::PayloadTooLarge(length));
        }
        if buffer.len() < HEADER_LEN + length {
            return Ok(None);
        }

        buffer.advance(HEADER_LEN);
        let payload = buffer.split_to(length).freeze();
        Self::from_payload(channel, payload).map(Some)
    }

    fn payload(&self) -> Bytes {
        match self {
            Self::Stdin(bytes) | Self::Stdout(bytes) | Self::Stderr(bytes) => bytes.clone(),
            Self::Resize(size) => Bytes::copy_from_slice(&size.to_wire()),
            Self::Status(status) => serde_json::to_vec(status)
                .expect("a status frame always serializes to JSON")
                .into(),
        }
    }

    fn from_payload(channel: Channel, payload: Bytes) -> Result<Self, FrameError> {
        let frame = match channel {
            Channel::Stdin => Self::Stdin(payload),
            Channel::Stdout => Self::Stdout(payload),
            Channel::Stderr => Self::Stderr(payload),
            Channel::Error => Self::Status(
                serde_json::from_slice(payload.as_ref()).map_err(FrameError::InvalidStatus)?,
            ),
            Channel::Resize => Self::Resize(TerminalSize::from_wire(&payload).ok_or(
                FrameError::InvalidPayload {
                    channel,
                    expected: TerminalSize::WIRE_LEN,
                    length: payload.len(),
                },
            )?),
        };

        Ok(frame)
    }
}

/// Decodes frames while enforcing the terminal [`Frame::Status`] rule that
/// [`Frame::decode`] cannot: the status frame is last, and its absence means the
/// stream was truncated.
#[derive(Debug, Default)]
pub(crate) struct FrameStream {
    buffer: BytesMut,
    finished: bool,
}

impl FrameStream {
    pub(crate) fn feed(&mut self, chunk: &[u8]) {
        self.buffer.extend_from_slice(chunk);
    }

    /// Returns the next frame, or `None` when more bytes are needed. A call after
    /// the status frame is an error, so readers stop at [`Frame::Status`].
    pub(crate) fn decode(&mut self) -> Result<Option<Frame>, FrameError> {
        if self.finished {
            return Err(FrameError::FrameAfterStatus);
        }

        let frame = Frame::decode(&mut self.buffer)?;
        if matches!(frame, Some(Frame::Status(_))) {
            self.finished = true;
        }

        Ok(frame)
    }

    pub(crate) const fn is_finished(&self) -> bool {
        self.finished
    }

    /// Errors if the stream ended before its status frame arrived.
    pub(crate) fn finish(self) -> Result<(), FrameError> {
        if self.finished {
            Ok(())
        } else {
            Err(FrameError::MissingStatus)
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum FrameError {
    #[error("unknown channel id: {0}")]
    UnknownChannel(u8),
    #[error("channel {channel:?} payload must be at least {expected} bytes, got {length}")]
    InvalidPayload {
        channel: Channel,
        expected: usize,
        length: usize,
    },
    #[error("status frame payload is not valid JSON")]
    InvalidStatus(#[source] serde_json::Error),
    #[error("frame payload of {0} bytes exceeds the per-frame limit")]
    PayloadTooLarge(usize),
    #[error("frame received after the terminal status frame")]
    FrameAfterStatus,
    #[error("stream ended without a terminal status frame")]
    MissingStatus,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples() -> Vec<Frame> {
        vec![
            Frame::Stdin(Bytes::from_static(b"in")),
            Frame::Stdout(Bytes::from_static(b"out")),
            Frame::Stderr(Bytes::from_static(b"err")),
            Frame::Resize(TerminalSize { rows: 24, cols: 80 }),
            Frame::Status(Status::Exited { code: 3 }),
        ]
    }

    #[test]
    fn channel_ids_are_wire_stable() {
        assert_eq!(Channel::Stdin.as_u8(), 0);
        assert_eq!(Channel::Stdout.as_u8(), 1);
        assert_eq!(Channel::Stderr.as_u8(), 2);
        assert_eq!(Channel::Error.as_u8(), 3);
        assert_eq!(Channel::Resize.as_u8(), 4);
    }

    #[test]
    fn round_trips_every_frame() {
        for frame in samples() {
            let mut buffer = BytesMut::from(frame.encode().as_ref());
            assert_eq!(Frame::decode(&mut buffer).unwrap(), Some(frame));
            assert!(buffer.is_empty());
        }
    }

    #[test]
    fn round_trips_every_status() {
        let statuses = [
            Status::Success,
            Status::Exited { code: 1 },
            Status::Failed {
                message: "spawn failed".to_owned(),
            },
        ];

        for status in statuses {
            let frame = Frame::Status(status);
            let mut buffer = BytesMut::from(frame.encode().as_ref());
            assert_eq!(Frame::decode(&mut buffer).unwrap(), Some(frame));
            assert!(buffer.is_empty());
        }
    }

    #[test]
    fn decodes_concatenated_frames() {
        let frames = samples();
        let mut buffer = BytesMut::new();
        for frame in &frames {
            buffer.extend_from_slice(&frame.encode());
        }

        for frame in frames {
            assert_eq!(Frame::decode(&mut buffer).unwrap(), Some(frame));
        }
        assert!(buffer.is_empty());
    }

    #[test]
    fn waits_until_a_frame_is_complete() {
        let encoded = Frame::Stdout(Bytes::from_static(b"hello")).encode();

        for prefix in 0..encoded.len() {
            let mut buffer = BytesMut::from(&encoded[..prefix]);
            assert_eq!(Frame::decode(&mut buffer).unwrap(), None);
            assert_eq!(buffer.len(), prefix, "an incomplete frame is left in place");
        }
    }

    #[test]
    fn rejects_an_unknown_channel() {
        let mut buffer = BytesMut::new();
        buffer.put_u8(5);
        buffer.put_u32(0);

        assert!(matches!(
            Frame::decode(&mut buffer),
            Err(FrameError::UnknownChannel(5))
        ));
    }

    #[test]
    fn rejects_a_control_frame_with_the_wrong_payload() {
        let mut buffer = BytesMut::new();
        buffer.put_u8(Channel::Resize.as_u8());
        buffer.put_u32(2);
        buffer.put_slice(&[0, 0]);

        assert!(matches!(
            Frame::decode(&mut buffer),
            Err(FrameError::InvalidPayload {
                channel: Channel::Resize,
                expected: 4,
                length: 2,
            })
        ));
    }

    #[test]
    fn rejects_an_oversized_payload() {
        let mut buffer = BytesMut::new();
        buffer.put_u8(Channel::Stdout.as_u8());
        buffer.put_u32(MAX_PAYLOAD_LEN as u32 + 1);

        assert!(matches!(
            Frame::decode(&mut buffer),
            Err(FrameError::PayloadTooLarge(_))
        ));
    }

    #[test]
    fn rejects_a_malformed_status_frame() {
        let mut buffer = BytesMut::new();
        buffer.put_u8(Channel::Error.as_u8());
        buffer.put_u32(3);
        buffer.put_slice(b"n/a");

        assert!(matches!(
            Frame::decode(&mut buffer),
            Err(FrameError::InvalidStatus(_))
        ));
    }

    #[test]
    fn accepts_a_stream_that_ends_with_a_status_frame() {
        let mut stream = FrameStream::default();
        stream.feed(&Frame::Stdout(Bytes::from_static(b"out")).encode());
        stream.feed(&Frame::Status(Status::Success).encode());

        assert_eq!(
            stream.decode().unwrap(),
            Some(Frame::Stdout(Bytes::from_static(b"out")))
        );
        assert!(!stream.is_finished());

        assert_eq!(
            stream.decode().unwrap(),
            Some(Frame::Status(Status::Success))
        );
        assert!(stream.is_finished());

        stream.finish().unwrap();
    }

    #[test]
    fn rejects_a_frame_after_the_status_frame() {
        let mut stream = FrameStream::default();
        stream.feed(&Frame::Status(Status::Success).encode());
        stream.feed(&Frame::Stdout(Bytes::from_static(b"late")).encode());

        assert!(matches!(stream.decode().unwrap(), Some(Frame::Status(_))));
        assert!(matches!(stream.decode(), Err(FrameError::FrameAfterStatus)));
    }

    #[test]
    fn rejects_a_stream_without_a_status_frame() {
        let mut stream = FrameStream::default();
        stream.feed(&Frame::Stderr(Bytes::from_static(b"truncated")).encode());

        assert!(matches!(stream.decode().unwrap(), Some(Frame::Stderr(_))));
        assert!(matches!(stream.finish(), Err(FrameError::MissingStatus)));
    }
}
