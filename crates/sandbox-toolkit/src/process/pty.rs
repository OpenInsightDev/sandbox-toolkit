//! A pty session runs over a WebSocket, so every message already carries its own
//! boundary and a frame needs no length prefix.
//!
//! The channel set is closed, so a connection never creates channels. It has no
//! stderr because a pty folds stderr into stdout.

#![expect(
    dead_code,
    reason = "consumed by the pty session handlers, which are not written yet"
)]

use bytes::{BufMut, Bytes, BytesMut};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::model::Status;

/// The discriminants are wire identifiers; reordering them changes the format.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Channel {
    Stdin = 0,
    /// With stderr folded in.
    Stdout = 1,
    /// The process outcome, carrying the exit code or an error.
    Exit = 3,
    Resize = 4,
    Close = 255,
}

impl Channel {
    pub(crate) const fn as_u8(self) -> u8 {
        self as u8
    }

    pub(crate) fn from_u8(id: u8) -> Result<Self, FrameError> {
        match id {
            0 => Ok(Self::Stdin),
            1 => Ok(Self::Stdout),
            3 => Ok(Self::Exit),
            4 => Ok(Self::Resize),
            255 => Ok(Self::Close),
            other => Err(FrameError::UnknownChannel(other)),
        }
    }

    pub(crate) const fn direction(self) -> Direction {
        match self {
            Self::Stdin | Self::Resize => Direction::ClientToServer,
            Self::Stdout | Self::Exit => Direction::ServerToClient,
            Self::Close => Direction::Bidirectional,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    ClientToServer,
    ServerToClient,
    Bidirectional,
}

/// Terminal geometry in character cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub(crate) struct TerminalSize {
    pub(crate) rows: u16,
    pub(crate) cols: u16,
}

impl TerminalSize {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Frame {
    Stdin(Bytes),
    Stdout(Bytes),
    /// Ends the output stream.
    Exit(Status),
    Resize(TerminalSize),
    Close,
}

impl Frame {
    pub(crate) const fn channel(&self) -> Channel {
        match self {
            Self::Stdin(_) => Channel::Stdin,
            Self::Stdout(_) => Channel::Stdout,
            Self::Exit(_) => Channel::Exit,
            Self::Resize(_) => Channel::Resize,
            Self::Close => Channel::Close,
        }
    }

    pub(crate) const fn direction(&self) -> Direction {
        self.channel().direction()
    }

    pub(crate) fn encode(&self) -> Bytes {
        let payload = self.payload();
        let mut message = BytesMut::with_capacity(1 + payload.len());
        message.put_u8(self.channel().as_u8());
        message.put_slice(&payload);
        message.freeze()
    }

    pub(crate) fn decode(message: &[u8]) -> Result<Self, FrameError> {
        let Some((&id, payload)) = message.split_first() else {
            return Err(FrameError::EmptyMessage);
        };

        Self::from_payload(Channel::from_u8(id)?, payload)
    }

    fn payload(&self) -> Bytes {
        match self {
            Self::Stdin(bytes) | Self::Stdout(bytes) => bytes.clone(),
            Self::Exit(status) => serde_json::to_vec(status)
                .expect("a status frame always serializes to JSON")
                .into(),
            Self::Resize(size) => Bytes::copy_from_slice(&size.to_wire()),
            Self::Close => Bytes::new(),
        }
    }

    fn from_payload(channel: Channel, payload: &[u8]) -> Result<Self, FrameError> {
        let frame = match channel {
            Channel::Stdin => Self::Stdin(Bytes::copy_from_slice(payload)),
            Channel::Stdout => Self::Stdout(Bytes::copy_from_slice(payload)),
            Channel::Exit => {
                Self::Exit(serde_json::from_slice(payload).map_err(FrameError::InvalidStatus)?)
            }
            Channel::Resize => Self::Resize(TerminalSize::from_wire(payload).ok_or(
                FrameError::InvalidPayload {
                    channel,
                    expected: TerminalSize::WIRE_LEN,
                    length: payload.len(),
                },
            )?),
            // A close signal carries no information, so any payload is ignored.
            Channel::Close => Self::Close,
        };

        Ok(frame)
    }
}

/// [`Frame::Exit`] is terminal for output, leaving only [`Frame::Close`], and
/// nothing follows [`Frame::Close`].
#[derive(Debug, Default)]
pub(crate) struct SessionFrames {
    exited: bool,
    closed: bool,
}

impl SessionFrames {
    pub(crate) fn decode(&mut self, message: &[u8]) -> Result<Frame, FrameError> {
        if self.closed {
            return Err(FrameError::FrameAfterClose);
        }

        let frame = Frame::decode(message)?;
        match &frame {
            Frame::Close => self.closed = true,
            Frame::Exit(_) if self.exited => return Err(FrameError::FrameAfterExit),
            Frame::Exit(_) => self.exited = true,
            _ if self.exited => return Err(FrameError::FrameAfterExit),
            _ => {}
        }

        Ok(frame)
    }

    pub(crate) const fn exited(&self) -> bool {
        self.exited
    }

    pub(crate) const fn is_closed(&self) -> bool {
        self.closed
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum FrameError {
    #[error("empty WebSocket message carries no channel byte")]
    EmptyMessage,
    #[error("unknown channel id: {0}")]
    UnknownChannel(u8),
    #[error("channel {channel:?} payload must be at least {expected} bytes, got {length}")]
    InvalidPayload {
        channel: Channel,
        expected: usize,
        length: usize,
    },
    #[error("exit frame payload is not valid JSON")]
    InvalidStatus(#[source] serde_json::Error),
    #[error("frame received after the terminal exit frame")]
    FrameAfterExit,
    #[error("frame received after the close frame")]
    FrameAfterClose,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples() -> Vec<Frame> {
        vec![
            Frame::Stdin(Bytes::from_static(b"in")),
            Frame::Stdout(Bytes::from_static(b"out")),
            Frame::Exit(Status::Exited { exit_code: 3 }),
            Frame::Resize(TerminalSize { rows: 24, cols: 80 }),
            Frame::Close,
        ]
    }

    #[test]
    fn channel_ids_are_wire_stable() {
        assert_eq!(Channel::Stdin.as_u8(), 0);
        assert_eq!(Channel::Stdout.as_u8(), 1);
        assert_eq!(Channel::Exit.as_u8(), 3);
        assert_eq!(Channel::Resize.as_u8(), 4);
        assert_eq!(Channel::Close.as_u8(), 255);
    }

    #[test]
    fn channels_carry_their_direction() {
        assert_eq!(Channel::Stdin.direction(), Direction::ClientToServer);
        assert_eq!(Channel::Resize.direction(), Direction::ClientToServer);
        assert_eq!(Channel::Stdout.direction(), Direction::ServerToClient);
        assert_eq!(Channel::Exit.direction(), Direction::ServerToClient);
        assert_eq!(Channel::Close.direction(), Direction::Bidirectional);
    }

    #[test]
    fn round_trips_every_frame() {
        for frame in samples() {
            assert_eq!(Frame::decode(&frame.encode()).unwrap(), frame);
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
            let frame = Frame::Exit(status);
            assert_eq!(Frame::decode(&frame.encode()).unwrap(), frame);
        }
    }

    #[test]
    fn a_message_is_a_whole_frame() {
        assert_eq!(
            Frame::Stdin(Bytes::from_static(b"hi")).encode().as_ref(),
            b"\x00hi"
        );
        assert_eq!(Frame::Close.encode().as_ref(), b"\xff");
    }

    #[test]
    fn exit_payload_is_json() {
        let status = Status::Exited { exit_code: 2 };
        let encoded = Frame::Exit(status.clone()).encode();

        assert_eq!(
            encoded.slice(1..).as_ref(),
            serde_json::to_vec(&status).unwrap()
        );
    }

    #[test]
    fn rejects_an_empty_message() {
        assert!(matches!(Frame::decode(&[]), Err(FrameError::EmptyMessage)));
    }

    #[test]
    fn rejects_a_stderr_channel() {
        // A pty folds stderr into stdout, so id 2 is not a channel.
        assert!(matches!(
            Frame::decode(&[2]),
            Err(FrameError::UnknownChannel(2))
        ));
    }

    #[test]
    fn rejects_an_unknown_channel() {
        assert!(matches!(
            Frame::decode(&[5]),
            Err(FrameError::UnknownChannel(5))
        ));
    }

    #[test]
    fn rejects_a_resize_payload_shorter_than_two_integers() {
        assert!(matches!(
            Frame::decode(&[Channel::Resize.as_u8(), 0, 0, 0]),
            Err(FrameError::InvalidPayload {
                channel: Channel::Resize,
                expected: 4,
                length: 3,
            })
        ));
    }

    #[test]
    fn accepts_a_resize_payload_with_trailing_bytes() {
        // The size is a minimum, so a later, larger encoding still decodes.
        let frame = Frame::decode(&[Channel::Resize.as_u8(), 0, 24, 0, 80, 9, 9]).unwrap();

        assert_eq!(frame, Frame::Resize(TerminalSize { rows: 24, cols: 80 }));
    }

    #[test]
    fn ignores_the_payload_of_a_close_signal() {
        assert_eq!(
            Frame::decode(&[Channel::Close.as_u8(), 1, 2]).unwrap(),
            Frame::Close
        );
    }

    #[test]
    fn rejects_a_malformed_exit_payload() {
        assert!(matches!(
            Frame::decode(&[Channel::Exit.as_u8(), b'n', b'/', b'a']),
            Err(FrameError::InvalidStatus(_))
        ));
    }

    #[test]
    fn only_close_follows_the_exit_frame() {
        let mut frames = SessionFrames::default();
        frames
            .decode(&Frame::Exit(Status::Exited { exit_code: 0 }).encode())
            .unwrap();
        assert!(frames.exited());

        assert!(matches!(
            frames.decode(&Frame::Stdout(Bytes::from_static(b"late")).encode()),
            Err(FrameError::FrameAfterExit)
        ));
        assert!(matches!(
            frames.decode(&Frame::Exit(Status::Exited { exit_code: 0 }).encode()),
            Err(FrameError::FrameAfterExit)
        ));

        // The connection still closes normally after the exit frame.
        frames.decode(&Frame::Close.encode()).unwrap();
        assert!(frames.is_closed());
    }

    #[test]
    fn nothing_follows_the_close_frame() {
        let mut frames = SessionFrames::default();
        frames
            .decode(&Frame::Stdout(Bytes::from_static(b"out")).encode())
            .unwrap();
        frames.decode(&Frame::Close.encode()).unwrap();

        assert!(matches!(
            frames.decode(&Frame::Stdin(Bytes::from_static(b"in")).encode()),
            Err(FrameError::FrameAfterClose)
        ));
        assert!(matches!(
            frames.decode(&Frame::Close.encode()),
            Err(FrameError::FrameAfterClose)
        ));
    }
}
