//! # cubeplane-protocol
//!
//! Low-level wire-format primitives for the Minecraft Java Edition protocol
//! (protocol **763**, game version **1.20.1**).
//!
//! This crate is deliberately runtime-agnostic: it knows how to turn bytes into
//! Minecraft values and back, and nothing else. Async framing, compression and
//! the actual TCP plumbing live in `cubeplane-server`.
//!
//! The design follows the "atlas" philosophy of small composable pieces:
//!
//! * [`read::ProtoRead`] / [`write::ProtoWrite`] — extension traits over
//!   `bytes::Buf` / `bytes::BufMut` for every Minecraft scalar type.
//! * [`packet::Encode`] / [`packet::Decode`] — per-packet (de)serialization.
//! * [`packet::State`] — the handshaking → status/login → play state machine.

pub mod error;
pub mod packet;
pub mod read;
pub mod write;

pub use error::{ProtocolError, Result};
pub use packet::{Decode, Encode, RawPacket, State};
pub use read::ProtoRead;
pub use write::{varint_len, ProtoWrite};

/// The protocol version implemented by cubeplane (Minecraft 1.20.1).
pub const PROTOCOL_VERSION: i32 = 763;

/// Human-readable game version string advertised in status responses.
pub const GAME_VERSION: &str = "1.20.1";

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::BytesMut;

    #[test]
    fn varint_roundtrip() {
        let cases = [0, 1, 127, 128, 255, 25565, -1, i32::MAX, i32::MIN, 2097151];
        for &v in &cases {
            let mut buf = BytesMut::new();
            buf.write_varint(v);
            let mut slice = buf.clone();
            assert_eq!(slice.read_varint().unwrap(), v, "varint {v}");
            assert_eq!(buf.len(), varint_len(v), "varint_len {v}");
        }
    }

    #[test]
    fn varlong_roundtrip() {
        let cases = [0i64, 1, 127, 128, i64::MAX, i64::MIN, -1, 9223372036854775807];
        for &v in &cases {
            let mut buf = BytesMut::new();
            buf.write_varlong(v);
            let mut slice = buf.clone();
            assert_eq!(slice.read_varlong().unwrap(), v, "varlong {v}");
        }
    }

    #[test]
    fn string_roundtrip() {
        let mut buf = BytesMut::new();
        buf.write_string("Hello, cubeplane! ⛏");
        let mut slice = buf.clone();
        assert_eq!(slice.read_string().unwrap(), "Hello, cubeplane! ⛏");
    }

    #[test]
    fn position_roundtrip() {
        let cases = [(0, 0, 0), (1, 2, 3), (-1, -1, -1), (1000, 64, -2000), (-30_000_000, -64, 30_000_000)];
        for &(x, y, z) in &cases {
            let mut buf = BytesMut::new();
            buf.write_position(x, y, z);
            let mut slice = buf.clone();
            assert_eq!(slice.read_position().unwrap(), (x, y, z), "pos {x},{y},{z}");
        }
    }
}
