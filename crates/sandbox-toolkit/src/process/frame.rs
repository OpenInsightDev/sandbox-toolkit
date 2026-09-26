//! The frame stream carried by an upgraded exec response. Frames are
//! length-prefixed rather than separated, because command output may contain any
//! byte a separator would use.
//!
//! Every stream ends with exactly one terminal [`Frame::Status`]; without it the
//! stream was truncated, not successful, which [`FrameStream`] enforces.

#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the frame decoders serve clients and tests, never the server"
    )
)]

use bytes::{Buf, BufMut, Bytes, BytesMut};
use thiserror::Error;

use super::model::Status;

pub(crate) const HEADER_LEN: usize = 5;

/// The peer controls the advertised length, so [`Frame::decode`] rejects anything
/// larger before allocating.
pub(crate) const MAX_PAYLOAD_LEN: usize = 4 * 1024 * 1024;

/// The exec stream is server to client only: an exec request carries no input and
/// no terminal, so the only channels are the two output streams and the terminal
/// status. The ids are shared with the pty session, whose stdin is id `0`; exec
/// reserves it because it carries no input.
///
/// The discriminants are wire identifiers; reordering them changes the format.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Channel {
    Stdout = 1,
    Stderr = 2,
    /// The terminal status, carrying success as well as failure.
    Error = 3,
}

impl Channel {
    pub(crate) const fn as_u8(self) -> u8 {
        self as u8
    }

    pub(crate) fn from_u8(id: u8) -> Result<Self, FrameError> {
        match id {
            1 => Ok(Self::Stdout),
            2 => Ok(Self::Stderr),
            3 => Ok(Self::Error),
            other => Err(FrameError::UnknownChannel(other)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Frame {
    Stdout(Bytes),
    Stderr(Bytes),
    /// On the error channel. Exactly one ends every stream, whether the command
    /// succeeded or failed.
    Status(Status),
}

impl Frame {
    pub(crate) const fn channel(&self) -> Channel {
        match self {
            Self::Stdout(_) => Channel::Stdout,
            Self::Stderr(_) => Channel::Stderr,
            Self::Status(_) => Channel::Error,
        }
    }

    pub(crate) fn payload_len(&self) -> usize {
        match self {
            Self::Stdout(bytes) | Self::Stderr(bytes) => bytes.len(),
            Self::Status(status) => serde_json::to_vec(status)
                .expect("a status frame always serializes to JSON")
                .len(),
        }
    }

    /// Splitting payloads above [`MAX_PAYLOAD_LEN`] is the producer's
    /// responsibility, since only the decoder enforces that bound.
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
            Self::Stdout(bytes) | Self::Stderr(bytes) => bytes.clone(),
            Self::Status(status) => serde_json::to_vec(status)
                .expect("a status frame always serializes to JSON")
                .into(),
        }
    }

    fn from_payload(channel: Channel, payload: Bytes) -> Result<Self, FrameError> {
        let frame = match channel {
            Channel::Stdout => Self::Stdout(payload),
            Channel::Stderr => Self::Stderr(payload),
            Channel::Error => Self::Status(
                serde_json::from_slice(payload.as_ref()).map_err(FrameError::InvalidStatus)?,
            ),
        };

        Ok(frame)
    }
}

/// Enforces the terminal [`Frame::Status`] rule that [`Frame::decode`] cannot: the
/// status frame is last, and its absence means the stream was truncated.
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

#[derive(Debug, Error)]
pub(crate) enum FrameError {
    #[error("unknown channel id: {0}")]
    UnknownChannel(u8),
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
            Frame::Stdout(Bytes::from_static(b"out")),
            Frame::Stderr(Bytes::from_static(b"err")),
            Frame::Status(Status::Exited { exit_code: 3 }),
        ]
    }

    #[test]
    fn channel_ids_are_wire_stable() {
        assert_eq!(Channel::Stdout.as_u8(), 1);
        assert_eq!(Channel::Stderr.as_u8(), 2);
        assert_eq!(Channel::Error.as_u8(), 3);
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
            Status::Exited { exit_code: 0 },
            Status::Exited { exit_code: 1 },
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
        // Id 0 is stdin in the shared numbering, which exec reserves and never uses.
        for id in [0, 4] {
            let mut buffer = BytesMut::new();
            buffer.put_u8(id);
            buffer.put_u32(0);

            assert!(matches!(
                Frame::decode(&mut buffer),
                Err(FrameError::UnknownChannel(unknown)) if unknown == id
            ));
        }
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
        stream.feed(&Frame::Status(Status::Exited { exit_code: 0 }).encode());

        assert_eq!(
            stream.decode().unwrap(),
            Some(Frame::Stdout(Bytes::from_static(b"out")))
        );
        assert!(!stream.is_finished());

        assert_eq!(
            stream.decode().unwrap(),
            Some(Frame::Status(Status::Exited { exit_code: 0 }))
        );
        assert!(stream.is_finished());

        stream.finish().unwrap();
    }

    #[test]
    fn rejects_a_frame_after_the_status_frame() {
        let mut stream = FrameStream::default();
        stream.feed(&Frame::Status(Status::Exited { exit_code: 0 }).encode());
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
