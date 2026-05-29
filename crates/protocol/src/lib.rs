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

/// The protocol version implemented by cubeplane for the play state
/// (Minecraft 1.20.1). This is the version whose packet layouts are wired up.
pub const PROTOCOL_VERSION: i32 = 763;

/// Human-readable game version string advertised in status responses.
pub const GAME_VERSION: &str = "1.20.1";

/// A known Minecraft Java version: its protocol number and display name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Version {
    pub protocol: i32,
    pub name: &'static str,
}

/// A registry of recent Minecraft Java versions, newest first. Used to name the
/// version a client connected with (even one we can't host) and to drive the
/// multi-version negotiation policy. Extending play support to another version
/// means adding its packet-id map alongside the entry here.
pub const KNOWN_VERSIONS: &[Version] = &[
    Version { protocol: 767, name: "1.21 / 1.21.1" },
    Version { protocol: 766, name: "1.20.5 / 1.20.6" },
    Version { protocol: 765, name: "1.20.3 / 1.20.4" },
    Version { protocol: 764, name: "1.20.2" },
    Version { protocol: 763, name: "1.20.1" },
    Version { protocol: 762, name: "1.19.4" },
    Version { protocol: 761, name: "1.19.3" },
    Version { protocol: 760, name: "1.19.1 / 1.19.2" },
    Version { protocol: 759, name: "1.19" },
    Version { protocol: 758, name: "1.18.2" },
    Version { protocol: 757, name: "1.18 / 1.18.1" },
    Version { protocol: 756, name: "1.17.1" },
    Version { protocol: 755, name: "1.17" },
    Version { protocol: 754, name: "1.16.4 / 1.16.5" },
    Version { protocol: 47, name: "1.8.x" },
];

/// The protocol versions cubeplane can host a play session for. 1.20.1 is the
/// canonical/native layout; 1.20.2 (764) is served through the translation layer
/// (id maps + body rewriters + the Configuration phase). This is the single
/// source of truth the login gate consults.
pub const SUPPORTED_PROTOCOLS: &[i32] = &[PROTOCOL_VERSION, 764, 762];

/// Whether cubeplane can host a play session for a client protocol version.
pub fn is_supported(protocol: i32) -> bool {
    SUPPORTED_PROTOCOLS.contains(&protocol)
}

/// The display name for a protocol version, if it's one we recognise.
pub fn version_name(protocol: i32) -> Option<&'static str> {
    KNOWN_VERSIONS.iter().find(|v| v.protocol == protocol).map(|v| v.name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::BytesMut;

    #[test]
    fn version_registry() {
        // The hosted version is recognised, named, and supported.
        assert!(is_supported(PROTOCOL_VERSION));
        assert_eq!(version_name(763), Some("1.20.1"));
        assert_eq!(version_name(764), Some("1.20.2"));
        // An unhosted-but-known version names cleanly but isn't supported.
        assert!(!is_supported(47));
        assert_eq!(version_name(47), Some("1.8.x"));
        // An entirely unknown protocol has no name and isn't supported.
        assert_eq!(version_name(99999), None);
        assert!(!is_supported(99999));
        // Every known version has a non-empty display name.
        for v in KNOWN_VERSIONS {
            assert!(!v.name.is_empty());
        }
    }

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
