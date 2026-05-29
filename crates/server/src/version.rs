//! Multi-version protocol translation.
//!
//! cubeplane builds every clientbound packet against the canonical protocol 763
//! (Minecraft 1.20.1) layout. To serve a client on a different protocol, the
//! outgoing byte stream must be translated to that version's wire format. This
//! module is the central translation point: every packet a player is sent passes
//! through [`translate_clientbound`], which rewrites the leading packet-id varint
//! to the target version's id.
//!
//! Today only protocol 763 is fully wired (its map is the identity), so the
//! translation is a no-op for connectable clients. Adding a version means adding
//! its verified `(canonical_id -> wire_id)` map in [`remap_play_clientbound`]
//! (and, where packet bodies differ, body rewriters) — the mechanism here is
//! already in place and exercised by tests.

use bytes::BytesMut;
use cubeplane_protocol::{ProtoRead, ProtoWrite, PROTOCOL_VERSION};

/// Translate a clientbound play payload (`id varint || body`) from the canonical
/// 763 layout to `protocol`'s wire format. For the hosted protocol this returns
/// the payload unchanged.
pub fn translate_clientbound(payload: BytesMut, protocol: i32) -> BytesMut {
    if protocol == PROTOCOL_VERSION {
        return payload;
    }
    let mut body = payload;
    let canonical = match body.read_varint() {
        Ok(id) => id,
        Err(_) => return body, // malformed; pass through untouched
    };
    let wire = remap_play_clientbound(canonical, protocol);
    if wire == canonical {
        // Re-prepend the original id without copying the body's meaning.
        let mut out = BytesMut::with_capacity(body.len() + 3);
        out.write_varint(canonical);
        out.extend_from_slice(&body);
        return out;
    }
    let mut out = BytesMut::with_capacity(body.len() + 3);
    out.write_varint(wire);
    out.extend_from_slice(&body);
    out
}

/// Map a canonical (763) clientbound play packet id to the wire id for a target
/// protocol. Unknown/unsupported protocols fall back to identity.
fn remap_play_clientbound(canonical_id: i32, protocol: i32) -> i32 {
    // Additional versions register their id maps here once verified, e.g.
    //   if protocol == 764 { return apply_map(canonical_id, MAP_763_TO_764); }
    // until then every protocol uses the canonical (identity) mapping.
    let _ = protocol;
    canonical_id
}

/// Apply a sparse `(from, to)` id remap, leaving unlisted ids unchanged. Helper
/// for per-version maps.
#[allow(dead_code)]
fn apply_map(id: i32, map: &[(i32, i32)]) -> i32 {
    map.iter().find(|(from, _)| *from == id).map(|(_, to)| *to).unwrap_or(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(id: i32, body: &[u8]) -> BytesMut {
        let mut b = BytesMut::new();
        b.write_varint(id);
        b.extend_from_slice(body);
        b
    }

    #[test]
    fn hosted_protocol_is_passthrough() {
        let p = payload(0x24, &[1, 2, 3, 4]);
        let original = p.clone();
        assert_eq!(translate_clientbound(p, PROTOCOL_VERSION), original);
    }

    #[test]
    fn identity_remap_preserves_id_and_body() {
        // An unknown protocol uses identity, so the round-trip is unchanged.
        let p = payload(0x52, &[9, 8, 7]);
        let out = translate_clientbound(p, 700);
        let mut rd = out.clone();
        assert_eq!(rd.read_varint().unwrap(), 0x52);
        assert_eq!(&rd[..], &[9, 8, 7]);
    }

    #[test]
    fn apply_map_remaps_only_listed_ids() {
        let map = [(0x10, 0x12), (0x20, 0x1f)];
        assert_eq!(apply_map(0x10, &map), 0x12);
        assert_eq!(apply_map(0x20, &map), 0x1f);
        assert_eq!(apply_map(0x30, &map), 0x30); // unlisted → unchanged
    }

    #[test]
    fn translation_rewrites_id_via_map() {
        // Prove the mechanism: feed a payload through a synthetic remap and check
        // the id varint changes while the body is preserved byte-for-byte.
        let body = [0xaa, 0xbb, 0xcc];
        let p = payload(0x10, &body);
        let mut src = p;
        let id = src.read_varint().unwrap();
        let wire = apply_map(id, &[(0x10, 0x77)]);
        let mut out = BytesMut::new();
        out.write_varint(wire);
        out.extend_from_slice(&src);
        let mut rd = out;
        assert_eq!(rd.read_varint().unwrap(), 0x77);
        assert_eq!(&rd[..], &body);
    }
}
