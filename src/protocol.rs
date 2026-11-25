//! Minecraft Java Edition Server List Ping (SLP) protocol.
//!
//! OWNERSHIP: Claude Sonnet (`feat/protocol`).
//! Public signatures below are the contract; bodies are to be implemented.
//! Extra private helpers / extra pub items are allowed.
//! Spec: https://minecraft.wiki/w/Java_Edition_protocol/Server_List_Ping

use std::time::Duration;

use thiserror::Error;

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
    let _ = (buf, value);
    todo!("feat/protocol")
}

/// Decode a VarInt from the beginning of `bytes`.
/// Returns `(value, bytes_consumed)`; `Ok(None)` if `bytes` ends before the
/// VarInt is complete.
pub fn read_varint(bytes: &[u8]) -> Result<Option<(i32, usize)>, ProtocolError> {
    let _ = bytes;
    todo!("feat/protocol")
}

/// Read a VarInt from an async stream.
pub async fn read_varint_async<R>(reader: &mut R) -> Result<i32, ProtocolError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let _ = reader;
    todo!("feat/protocol")
}

/// Build the complete (length-prefixed) Handshake packet with next_state = 1.
pub fn build_handshake(host: &str, port: u16) -> Vec<u8> {
    let _ = (host, port);
    todo!("feat/protocol")
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
    let _ = (connect_host, connect_port, handshake_host, handshake_port, timeout);
    todo!("feat/protocol")
}
