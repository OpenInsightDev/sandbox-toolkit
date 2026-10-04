use axum::extract::ws::Message;
use bytes::Bytes;
use pty::TerminalSize;

use crate::exec::frame::{ERROR, MAX_PAYLOAD_LEN, RESIZE, STDIN, STDOUT, Status};

pub enum Client {
    Stdin(Vec<u8>),
    Resize(TerminalSize),
}

pub fn decode(message: &[u8]) -> Option<Client> {
    let (channel, payload) = message.split_first()?;
    if payload.len() > MAX_PAYLOAD_LEN {
        return None;
    }

    match *channel {
        STDIN => Some(Client::Stdin(payload.to_vec())),
        RESIZE => Some(Client::Resize(size(payload)?)),
        _ => None,
    }
}

pub fn stdout(bytes: &[u8]) -> impl Iterator<Item = Message> + '_ {
    bytes
        .chunks(MAX_PAYLOAD_LEN)
        .map(|chunk| carried(STDOUT, chunk))
}

pub fn terminal(status: &Status) -> Message {
    carried(ERROR, &status.payload())
}

fn carried(channel: u8, payload: &[u8]) -> Message {
    let mut message = Vec::with_capacity(payload.len() + 1);
    message.push(channel);
    message.extend_from_slice(payload);

    Message::Binary(Bytes::from(message))
}

/// The terminal size a `resize` payload carries: two big-endian rows, then two
/// big-endian columns.
fn size(payload: &[u8]) -> Option<TerminalSize> {
    let [rows_high, rows_low, cols_high, cols_low] = payload else {
        return None;
    };

    Some(TerminalSize {
        rows: u16::from_be_bytes([*rows_high, *rows_low]),
        cols: u16::from_be_bytes([*cols_high, *cols_low]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(message: Message) -> (u8, Vec<u8>) {
        let Message::Binary(bytes) = message else {
            panic!("not a binary message");
        };
        let (channel, payload) = bytes.split_first().expect("a channel byte");

        (*channel, payload.to_vec())
    }

    #[test]
    fn stdin_and_resize_are_the_client_channels() {
        assert!(
            matches!(decode(&[STDIN, b'h', b'i']), Some(Client::Stdin(bytes)) if bytes == b"hi")
        );

        let Some(Client::Resize(size)) = decode(&[RESIZE, 0, 31, 0, 101]) else {
            panic!("resize is decoded");
        };
        assert_eq!((31, 101), (size.rows, size.cols));
    }

    #[test]
    fn a_message_that_breaks_the_contract_is_rejected() {
        assert!(decode(&[]).is_none(), "a channel byte is required");
        for channel in [STDOUT, crate::exec::frame::STDERR, ERROR] {
            assert!(
                decode(&[channel, b'x']).is_none(),
                "{channel} is server only"
            );
        }
        assert!(
            decode(&[RESIZE, 0, 31, 0]).is_none(),
            "resize carries four bytes"
        );
        assert!(
            decode(&[RESIZE, 0, 31, 0, 101, 0]).is_none(),
            "resize carries four bytes"
        );
    }

    #[test]
    fn a_payload_past_the_limit_is_rejected() {
        let mut message = vec![STDIN];
        message.resize(MAX_PAYLOAD_LEN + 2, b'a');

        assert!(decode(&message).is_none());
    }

    #[test]
    fn output_is_split_to_the_payload_limit() {
        let output = vec![b'a'; MAX_PAYLOAD_LEN + 1];
        let lengths = stdout(&output)
            .map(|message| {
                let (channel, payload) = split(message);
                assert_eq!(STDOUT, channel);
                payload.len()
            })
            .collect::<Vec<_>>();

        assert_eq!(vec![MAX_PAYLOAD_LEN, 1], lengths);
    }

    #[test]
    fn the_terminal_status_is_the_error_channel() {
        let (channel, payload) = split(terminal(&Status::Exited { exit_code: 4 }));

        assert_eq!(ERROR, channel);
        assert_eq!(
            serde_json::json!({ "status": "exited", "exit_code": 4 }),
            serde_json::from_slice::<serde_json::Value>(&payload).expect("the status is JSON")
        );
    }
}
