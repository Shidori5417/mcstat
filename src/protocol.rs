//! Minecraft Java Edition Server List Ping (SLP) protocol.
//!
//! OWNERSHIP: Claude Sonnet (`feat/protocol`).
//! Public signatures below are the contract; bodies are to be implemented.
//! Extra private helpers / extra pub items are allowed.
//! Spec: https://minecraft.wiki/w/Java_Edition_protocol/Server_List_Ping

use std::time::Duration;

use thiserror::Error;
use tokio::io::AsyncReadExt;

use crate::model::ServerStatus;

/// Protocol version sent in the handshake. -1 / any value works for status;
/// a recent real value is used for maximum compatibility.
pub const HANDSHAKE_PROTOCOL_VERSION: i32 = 769;

/// Upper bound on the status JSON packet size we accept (guards memory).
pub const MAX_PACKET_LEN: usize = 2 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("connection timed out")]
    Timeout,
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("VarInt is too big")]
    VarIntTooBig,
    #[error("packet too large ({0} bytes)")]
    PacketTooLarge(usize),
    #[error("unexpected packet id {0:#04x}")]
    UnexpectedPacket(i32),
    #[error("invalid status JSON: {0}")]
    InvalidJson(#[from] serde_json::Error),
    #[error("malformed response: {0}")]
    Malformed(String),
}

/// Encode `value` as a Minecraft VarInt and append it to `buf`.
pub fn write_varint(buf: &mut Vec<u8>, value: i32) {
    // Reinterpret as unsigned so negative numbers use the full 5 bytes.
    let mut v = value as u32;
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            buf.push(byte);
            return;
        }
        buf.push(byte | 0x80);
    }
}

/// Maximum number of bytes a VarInt (32-bit) may occupy.
const VARINT_MAX_BYTES: usize = 5;

/// Fold the `index`-th VarInt byte into `acc`; returns `true` when the byte
/// was the last one (continuation bit clear).
fn varint_step(acc: &mut u32, index: usize, byte: u8) -> bool {
    *acc |= u32::from(byte & 0x7f) << (7 * index);
    byte & 0x80 == 0
}

/// Decode a VarInt from the beginning of `bytes`.
/// Returns `(value, bytes_consumed)`; `Ok(None)` if `bytes` ends before the
/// VarInt is complete.
pub fn read_varint(bytes: &[u8]) -> Result<Option<(i32, usize)>, ProtocolError> {
    let mut acc = 0u32;
    for index in 0..VARINT_MAX_BYTES {
        let Some(&byte) = bytes.get(index) else {
            return Ok(None);
        };
        if varint_step(&mut acc, index, byte) {
            return Ok(Some((acc as i32, index + 1)));
        }
    }
    Err(ProtocolError::VarIntTooBig)
}

/// Read a VarInt from an async stream.
pub async fn read_varint_async<R>(reader: &mut R) -> Result<i32, ProtocolError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut acc = 0u32;
    for index in 0..VARINT_MAX_BYTES {
        let byte = reader.read_u8().await?;
        if varint_step(&mut acc, index, byte) {
            return Ok(acc as i32);
        }
    }
    Err(ProtocolError::VarIntTooBig)
}

/// Build the complete (length-prefixed) Handshake packet with next_state = 1.
pub fn build_handshake(host: &str, port: u16) -> Vec<u8> {
    let mut payload = Vec::with_capacity(host.len() + 10);
    write_varint(&mut payload, 0x00); // packet id: Handshake
    write_varint(&mut payload, HANDSHAKE_PROTOCOL_VERSION);
    write_string(&mut payload, host);
    payload.extend_from_slice(&port.to_be_bytes());
    write_varint(&mut payload, 1); // next state: Status

    let mut packet = Vec::with_capacity(payload.len() + 5);
    write_varint(
        &mut packet,
        i32::try_from(payload.len()).unwrap_or(i32::MAX),
    );
    packet.extend_from_slice(&payload);
    packet
}

/// Append a Minecraft String (VarInt byte length + UTF-8 bytes).
fn write_string(buf: &mut Vec<u8>, value: &str) {
    write_varint(buf, i32::try_from(value.len()).unwrap_or(i32::MAX));
    buf.extend_from_slice(value.as_bytes());
}

/// Parse the status JSON (Status Response payload) into a [`ServerStatus`].
/// `latency` and `resolved_endpoint` are filled from the arguments.
/// Must be lenient: missing `players`, `sample`, `favicon` etc. are allowed;
/// `description` may be a string or a chat component object.
pub fn parse_status_json(
    json: &str,
    latency: Duration,
    resolved_endpoint: &str,
) -> Result<ServerStatus, ProtocolError> {
    let _ = (json, latency, resolved_endpoint);
    todo!("feat/protocol")
}

/// Perform one complete SLP exchange:
/// TCP connect to `connect_host:connect_port`, send Handshake (using
/// `handshake_host`/`handshake_port`, which differ from the connect target
/// when an SRV record was followed), Status Request, read Status Response,
/// then Ping/Pong to measure latency.
///
/// The WHOLE exchange is bounded by `timeout` (returns
/// [`ProtocolError::Timeout`]). If the server closes the connection or does
/// not answer the ping, latency falls back to the status round-trip.
pub async fn query_status(
    connect_host: &str,
    connect_port: u16,
    handshake_host: &str,
    handshake_port: u16,
    timeout: Duration,
) -> Result<ServerStatus, ProtocolError> {
    let _ = (
        connect_host,
        connect_port,
        handshake_host,
        handshake_port,
        timeout,
    );
    todo!("feat/protocol")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (value, encoded bytes) vectors from the wiki.
    const VARINT_VECTORS: &[(i32, &[u8])] = &[
        (0, &[0x00]),
        (1, &[0x01]),
        (127, &[0x7f]),
        (128, &[0x80, 0x01]),
        (255, &[0xff, 0x01]),
        (25565, &[0xdd, 0xc7, 0x01]),
        (2097151, &[0xff, 0xff, 0x7f]),
        (2147483647, &[0xff, 0xff, 0xff, 0xff, 0x07]),
        (-1, &[0xff, 0xff, 0xff, 0xff, 0x0f]),
        (-2147483648, &[0x80, 0x80, 0x80, 0x80, 0x08]),
    ];

    #[test]
    fn varint_encodes_wiki_vectors() {
        for &(value, bytes) in VARINT_VECTORS {
            let mut buf = Vec::new();
            write_varint(&mut buf, value);
            assert_eq!(buf, bytes, "encoding {value}");
        }
    }

    #[test]
    fn varint_round_trip() {
        for &(value, bytes) in VARINT_VECTORS {
            let decoded = read_varint(bytes).expect("valid");
            assert_eq!(decoded, Some((value, bytes.len())), "decoding {value}");
        }
    }

    #[test]
    fn varint_ignores_trailing_bytes() {
        let decoded = read_varint(&[0x80, 0x01, 0xaa, 0xbb]).expect("valid");
        assert_eq!(decoded, Some((128, 2)));
    }

    #[test]
    fn varint_incomplete_returns_none() {
        assert_eq!(read_varint(&[]).expect("ok"), None);
        assert_eq!(read_varint(&[0x80]).expect("ok"), None);
        assert_eq!(read_varint(&[0xff, 0xff, 0xff, 0xff]).expect("ok"), None);
    }

    #[test]
    fn varint_too_big_is_rejected() {
        let err = read_varint(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x01]).expect_err("too big");
        assert!(matches!(err, ProtocolError::VarIntTooBig));
        let err = read_varint(&[0xff; 5]).expect_err("too big");
        assert!(matches!(err, ProtocolError::VarIntTooBig));
    }

    #[tokio::test]
    async fn varint_async_round_trip() {
        for &(value, bytes) in VARINT_VECTORS {
            let mut reader = bytes;
            let decoded = read_varint_async(&mut reader).await.expect("valid");
            assert_eq!(decoded, value);
            assert!(reader.is_empty(), "consumed exactly the VarInt");
        }
    }

    #[tokio::test]
    async fn varint_async_errors() {
        let mut short: &[u8] = &[0x80];
        let err = read_varint_async(&mut short).await.expect_err("eof");
        assert!(matches!(err, ProtocolError::Io(_)));

        let mut long: &[u8] = &[0xff; 6];
        let err = read_varint_async(&mut long).await.expect_err("too big");
        assert!(matches!(err, ProtocolError::VarIntTooBig));
    }

    #[test]
    fn handshake_byte_layout() {
        let packet = build_handshake("mc.example.com", 25565);
        let mut expected_payload = vec![0x00]; // packet id
        write_varint(&mut expected_payload, HANDSHAKE_PROTOCOL_VERSION);
        expected_payload.push(14); // host length
        expected_payload.extend_from_slice(b"mc.example.com");
        expected_payload.extend_from_slice(&[0x63, 0xdd]); // 25565 big endian
        expected_payload.push(0x01); // next state = status

        let mut expected = Vec::new();
        write_varint(&mut expected, expected_payload.len() as i32);
        expected.extend_from_slice(&expected_payload);
        assert_eq!(packet, expected);
        // Spot-check hard-coded bytes: 769 = [0x81, 0x06].
        assert_eq!(
            &packet[..5],
            &[expected_payload.len() as u8, 0x00, 0x81, 0x06, 14]
        );
        assert_eq!(&packet[packet.len() - 3..], &[0x63, 0xdd, 0x01]);
    }

    #[test]
    fn handshake_uses_utf8_byte_length() {
        let packet = build_handshake("sünucu.example", 1);
        // "sünucu.example" is 15 bytes (ü = 2 bytes), 14 chars.
        assert_eq!(packet[4], 15);
    }
}
