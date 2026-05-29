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
    // Rewrite the body for packets whose field layout changed in the target
    // version (e.g. 1.20.2's nameless network NBT). Identity when unchanged.
    let rewritten = rewrite_clientbound_body(canonical, protocol, &body);
    let body: &[u8] = rewritten.as_deref().unwrap_or(&body);
    let mut out = BytesMut::with_capacity(body.len() + 3);
    out.write_varint(wire);
    out.extend_from_slice(body);
    out
}

/// Rewrite a clientbound play packet body from the canonical 763 layout to a
/// target protocol's. Returns `Some(new_body)` when a rewrite applied, `None`
/// when the body is already wire-compatible.
fn rewrite_clientbound_body(canonical_id: i32, protocol: i32, body: &[u8]) -> Option<Vec<u8>> {
    if protocol != PROTO_1_20_2 {
        return None;
    }
    match canonical_id {
        // login / Join Game (763 0x28): reorder fields, drop the inline codec
        // (sent during the Configuration phase in 1.20.2), add doLimitedCrafting.
        0x28 => rewrite_login_763_to_764(body),
        // unload_chunk (763 0x1e): 1.20.2 swapped the field order to (z, x).
        0x1e => {
            if body.len() != 8 {
                return None;
            }
            let mut out = Vec::with_capacity(8);
            out.extend_from_slice(&body[4..8]); // chunkZ first
            out.extend_from_slice(&body[0..4]); // then chunkX
            Some(out)
        }
        // map_chunk (763 0x24): x:i32, z:i32, heightmaps:NBT, ... — 1.20.2 makes
        // the heightmaps a nameless network NBT.
        0x24 => {
            if body.len() < 8 {
                return None;
            }
            let mut out = Vec::with_capacity(body.len());
            out.extend_from_slice(&body[0..8]);
            let consumed = named_root_nbt_to_anonymous(&body[8..], &mut out)?;
            out.extend_from_slice(&body[8 + consumed..]);
            Some(out)
        }
        _ => None,
    }
}

/// Rewrite the 763 Join Game body into the 1.20.2 layout: the dimension codec is
/// removed (it ships in the Configuration phase), `doLimitedCrafting` is added,
/// and several fields are reordered.
fn rewrite_login_763_to_764(body: &[u8]) -> Option<Vec<u8>> {
    use cubeplane_protocol::{ProtoRead, ProtoWrite};
    let mut r = BytesMut::from(body);
    // --- read the 763 layout ---
    let entity_id = r.read_i32().ok()?;
    let hardcore = r.read_bool().ok()?;
    let game_mode = r.read_u8().ok()?;
    let prev_game_mode = r.read_i8().ok()?;
    let world_count = r.read_varint().ok()?;
    let mut worlds = Vec::new();
    for _ in 0..world_count {
        worlds.push(r.read_string().ok()?);
    }
    // Skip the inline dimension codec (a named root NBT compound).
    let codec_len = named_root_nbt_to_anonymous(&r, &mut Vec::new())?;
    let _ = r.split_to(codec_len);
    let world_type = r.read_string().ok()?;
    let world_name = r.read_string().ok()?;
    let hashed_seed = r.read_i64().ok()?;
    let max_players = r.read_varint().ok()?;
    let view_distance = r.read_varint().ok()?;
    let sim_distance = r.read_varint().ok()?;
    let reduced_debug = r.read_bool().ok()?;
    let respawn_screen = r.read_bool().ok()?;
    let is_debug = r.read_bool().ok()?;
    let is_flat = r.read_bool().ok()?;
    let has_death = r.read_bool().ok()?;
    // (763 has no death payload when has_death is false, matching our builder.)
    let portal_cooldown = r.read_varint().ok()?;

    // --- write the 764 layout ---
    let mut o = BytesMut::new();
    o.write_i32(entity_id);
    o.write_bool(hardcore);
    o.write_varint(world_count);
    for w in &worlds {
        o.write_string(w);
    }
    o.write_varint(max_players);
    o.write_varint(view_distance);
    o.write_varint(sim_distance);
    o.write_bool(reduced_debug);
    o.write_bool(respawn_screen);
    o.write_bool(false); // doLimitedCrafting (new in 1.20.2)
    o.write_string(&world_type);
    o.write_string(&world_name);
    o.write_i64(hashed_seed);
    o.write_u8(game_mode);
    o.write_i8(prev_game_mode);
    o.write_bool(is_debug);
    o.write_bool(is_flat);
    o.write_bool(has_death);
    o.write_varint(portal_cooldown);
    Some(o.to_vec())
}

/// Translate an inbound serverbound play payload from `protocol`'s wire format
/// to the canonical 763 layout the parser expects, by rewriting the leading
/// packet-id varint. The mirror of [`translate_clientbound`].
pub fn translate_serverbound(payload: BytesMut, protocol: i32) -> BytesMut {
    if protocol == PROTOCOL_VERSION {
        return payload;
    }
    let mut body = payload;
    let wire = match body.read_varint() {
        Ok(id) => id,
        Err(_) => return body,
    };
    let canonical = remap_play_serverbound(wire, protocol);
    let mut out = BytesMut::with_capacity(body.len() + 3);
    out.write_varint(canonical);
    out.extend_from_slice(&body);
    out
}

/// Map a target protocol's serverbound play wire id back to the canonical (763)
/// id. Unknown/unsupported protocols fall back to identity.
fn remap_play_serverbound(wire_id: i32, protocol: i32) -> i32 {
    match protocol {
        PROTO_1_20_2 => apply_map(wire_id, SB_764_TO_763),
        _ => wire_id,
    }
}

/// Map a canonical (763) clientbound play packet id to the wire id for a target
/// protocol. Unknown/unsupported protocols fall back to identity.
fn remap_play_clientbound(canonical_id: i32, protocol: i32) -> i32 {
    match protocol {
        PROTO_1_20_2 => apply_map(canonical_id, CB_763_TO_764),
        _ => canonical_id,
    }
}

/// Protocol number for Minecraft 1.20.2.
const PROTO_1_20_2: i32 = 764;

// Verified play packet-id maps between protocol 763 (1.20.1) and 764 (1.20.2),
// generated from PrismarineJS minecraft-data (pc/1.20 and pc/1.20.2). These cover
// every packet common to both versions whose id differs; packets unique to one
// version (e.g. start_configuration, chunk batching, named_entity_spawn) need
// body-level handling and are not part of the id remap.
const CB_763_TO_764: &[(i32, i32)] = &[
    (0x4, 0x3), (0x5, 0x4), (0x6, 0x5), (0x7, 0x6), (0x8, 0x7), (0x9, 0x8), (0xa, 0x9),
    (0xb, 0xa), (0xc, 0xb), (0xd, 0xe), (0xe, 0xf), (0xf, 0x10), (0x10, 0x11), (0x11, 0x12),
    (0x12, 0x13), (0x13, 0x14), (0x14, 0x15), (0x15, 0x16), (0x16, 0x17), (0x17, 0x18),
    (0x18, 0x19), (0x19, 0x1a), (0x1a, 0x1b), (0x1b, 0x1c), (0x1c, 0x1d), (0x1d, 0x1e),
    (0x1e, 0x1f), (0x1f, 0x20), (0x20, 0x21), (0x21, 0x22), (0x22, 0x23), (0x23, 0x24),
    (0x24, 0x25), (0x25, 0x26), (0x26, 0x27), (0x27, 0x28), (0x28, 0x29), (0x29, 0x2a),
    (0x2a, 0x2b), (0x2b, 0x2c), (0x2c, 0x2d), (0x2d, 0x2e), (0x2e, 0x2f), (0x2f, 0x30),
    (0x30, 0x31), (0x31, 0x32), (0x32, 0x33), (0x33, 0x35), (0x34, 0x36), (0x35, 0x37),
    (0x36, 0x38), (0x37, 0x39), (0x38, 0x3a), (0x39, 0x3b), (0x3a, 0x3c), (0x3b, 0x3d),
    (0x3c, 0x3e), (0x3d, 0x3f), (0x3e, 0x40), (0x3f, 0x41), (0x40, 0x42), (0x41, 0x43),
    (0x42, 0x44), (0x43, 0x45), (0x44, 0x46), (0x45, 0x47), (0x46, 0x48), (0x47, 0x49),
    (0x48, 0x4a), (0x49, 0x4b), (0x4a, 0x4c), (0x4b, 0x4d), (0x4c, 0x4e), (0x4d, 0x4f),
    (0x4e, 0x50), (0x4f, 0x51), (0x50, 0x52), (0x51, 0x53), (0x52, 0x54), (0x53, 0x55),
    (0x54, 0x56), (0x55, 0x57), (0x56, 0x58), (0x57, 0x59), (0x58, 0x5a), (0x59, 0x5b),
    (0x5a, 0x5c), (0x5b, 0x5d), (0x5c, 0x5e), (0x5d, 0x5f), (0x5e, 0x60), (0x5f, 0x61),
    (0x60, 0x62), (0x61, 0x63), (0x62, 0x64), (0x63, 0x66), (0x64, 0x67), (0x65, 0x68),
    (0x66, 0x69), (0x67, 0x6a), (0x68, 0x6b), (0x69, 0x6c), (0x6a, 0x6d), (0x6c, 0x6e),
    (0x6d, 0x6f), (0x6e, 0x70),
];

const SB_764_TO_763: &[(i32, i32)] = &[
    (0x8, 0x7), (0x9, 0x8), (0xa, 0x9), (0xc, 0xa), (0xd, 0xb), (0xe, 0xc), (0xf, 0xd),
    (0x10, 0xe), (0x11, 0xf), (0x12, 0x10), (0x13, 0x11), (0x14, 0x12), (0x15, 0x13),
    (0x16, 0x14), (0x17, 0x15), (0x18, 0x16), (0x19, 0x17), (0x1a, 0x18), (0x1b, 0x19),
    (0x1c, 0x1a), (0x1e, 0x1b), (0x1f, 0x1c), (0x20, 0x1d), (0x21, 0x1e), (0x22, 0x1f),
    (0x23, 0x20), (0x24, 0x21), (0x25, 0x22), (0x26, 0x23), (0x27, 0x24), (0x28, 0x25),
    (0x29, 0x26), (0x2a, 0x27), (0x2b, 0x28), (0x2c, 0x29), (0x2d, 0x2a), (0x2e, 0x2b),
    (0x2f, 0x2c), (0x30, 0x2d), (0x31, 0x2e), (0x32, 0x2f), (0x33, 0x30), (0x34, 0x31),
    (0x35, 0x32),
];

/// Apply a sparse `(from, to)` id remap, leaving unlisted ids unchanged. Helper
/// for per-version maps.
fn apply_map(id: i32, map: &[(i32, i32)]) -> i32 {
    map.iter().find(|(from, _)| *from == id).map(|(_, to)| *to).unwrap_or(id)
}

/// Convert a *named* root NBT compound (the 763 wire form: `0x0A` tag, a u16
/// name length, the name bytes, then the payload) into the *anonymous* network
/// NBT form 1.20.2+ uses (`0x0A` tag immediately followed by the payload, no
/// name). This is the keystone of 763→764 body translation: every packet NBT
/// field — chunk heightmaps, item-slot tags, the registry codec — changed to
/// this nameless form in 1.20.2. Returns the number of bytes consumed and writes
/// the converted compound to `out`.
///
/// `TAG_End` (`0x00`, an empty/absent compound) passes through unchanged.
pub fn named_root_nbt_to_anonymous(buf: &[u8], out: &mut Vec<u8>) -> Option<usize> {
    match buf.first()? {
        0x00 => {
            out.push(0x00); // empty NBT marker, identical in both forms
            Some(1)
        }
        0x0a => {
            // 0x0A tag, u16 big-endian name length, name bytes, then payload.
            let name_len = u16::from_be_bytes([*buf.get(1)?, *buf.get(2)?]) as usize;
            let payload_start = 3 + name_len;
            if buf.len() < payload_start {
                return None;
            }
            // The remaining payload is a compound body terminated by TAG_End; we
            // copy from here to the matching end of the compound.
            let consumed_payload = compound_body_len(&buf[payload_start..])?;
            out.push(0x0a); // anonymous compound: tag with no name
            out.extend_from_slice(&buf[payload_start..payload_start + consumed_payload]);
            Some(payload_start + consumed_payload)
        }
        _ => None, // not a root compound
    }
}

/// Length in bytes of a compound's body (its child tags up to and including the
/// closing `TAG_End`), starting at the first child tag.
fn compound_body_len(buf: &[u8]) -> Option<usize> {
    let mut i = 0;
    loop {
        let tag = *buf.get(i)?;
        i += 1;
        if tag == 0x00 {
            return Some(i); // TAG_End closes the compound
        }
        // Named child: u16 name length + name, then a payload of the tag's type.
        let name_len = u16::from_be_bytes([*buf.get(i)?, *buf.get(i + 1)?]) as usize;
        i += 2 + name_len;
        i += payload_len(tag, &buf[i..])?;
    }
}

/// Length of a tag payload of the given type id, given the bytes that follow.
fn payload_len(tag: u8, buf: &[u8]) -> Option<usize> {
    Some(match tag {
        1 => 1,                                          // byte
        2 => 2,                                          // short
        3 | 5 => 4,                                      // int / float
        4 | 6 => 8,                                      // long / double
        7 => 4 + i32::from_be_bytes(buf.get(0..4)?.try_into().ok()?).max(0) as usize, // byte array
        8 => 2 + u16::from_be_bytes([*buf.first()?, *buf.get(1)?]) as usize, // string
        9 => {
            // list: element type, i32 count, then count payloads.
            let elem = *buf.first()?;
            let count = i32::from_be_bytes(buf.get(1..5)?.try_into().ok()?).max(0) as usize;
            let mut i = 5;
            for _ in 0..count {
                i += payload_len(elem, &buf[i..])?;
            }
            i
        }
        10 => compound_body_len(buf)?,                   // nested compound
        11 => 4 + 4 * i32::from_be_bytes(buf.get(0..4)?.try_into().ok()?).max(0) as usize, // int array
        12 => 4 + 8 * i32::from_be_bytes(buf.get(0..4)?.try_into().ok()?).max(0) as usize, // long array
        _ => return None,
    })
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
    fn serverbound_passthrough_and_identity() {
        let p = payload(0x14, &[5, 6]);
        let original = p.clone();
        assert_eq!(translate_serverbound(p, PROTOCOL_VERSION), original);
        // Unknown protocol uses identity in both directions.
        let p = payload(0x14, &[5, 6]);
        let mut rd = translate_serverbound(p, 700);
        assert_eq!(rd.read_varint().unwrap(), 0x14);
        assert_eq!(&rd[..], &[5, 6]);
    }

    #[test]
    fn verified_764_clientbound_mappings() {
        // Spot-check against authoritative minecraft-data (pc/1.20 vs pc/1.20.2).
        // advancements: 763 0x69 -> 764 0x6c.
        assert_eq!(remap_play_clientbound(0x69, PROTO_1_20_2), 0x6c);
        // statistics: 763 0x05 -> 764 0x04.
        assert_eq!(remap_play_clientbound(0x05, PROTO_1_20_2), 0x04);
        // sound_effect: 763 0x62 -> 764 0x64.
        assert_eq!(remap_play_clientbound(0x62, PROTO_1_20_2), 0x64);
        // spawn_entity (0x01) is unchanged between the two versions.
        assert_eq!(remap_play_clientbound(0x01, PROTO_1_20_2), 0x01);
    }

    #[test]
    fn verified_764_serverbound_mappings() {
        // client_command: 764 0x08 -> 763 0x07.
        assert_eq!(remap_play_serverbound(0x08, PROTO_1_20_2), 0x07);
        // window_click: 764 0x0d -> 763 0x0b.
        assert_eq!(remap_play_serverbound(0x0d, PROTO_1_20_2), 0x0b);
        // teleport_confirm (0x00) is unchanged.
        assert_eq!(remap_play_serverbound(0x00, PROTO_1_20_2), 0x00);
    }

    #[test]
    fn nbt_named_to_anonymous_strips_root_name() {
        // Build a realistic named root compound with several tag types.
        let named = cubeplane_nbt::Nbt::compound()
            .put_int("Foo", 5)
            .put_string("Bar", "hi")
            .put_long_array("Nums", vec![7, 8, 9])
            .put_compound("Sub", cubeplane_nbt::Nbt::compound().put_byte("Q", 2))
            .to_bytes_named("Root");
        let mut out = Vec::new();
        let consumed = named_root_nbt_to_anonymous(&named, &mut out).unwrap();
        // The whole compound is consumed, nothing more.
        assert_eq!(consumed, named.len());
        // Result is the anonymous form: 0x0A tag, then the payload with the
        // 2-byte length + 4-byte "Root" name removed.
        assert_eq!(out[0], 0x0a);
        assert_eq!(&out[1..], &named[3 + 4..]);
    }

    #[test]
    fn nbt_converter_consumes_only_the_compound() {
        let named = cubeplane_nbt::Nbt::compound().put_byte("B", 1).to_bytes_named("X");
        let mut framed = named.clone();
        framed.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef]); // trailing bytes
        let mut out = Vec::new();
        let consumed = named_root_nbt_to_anonymous(&framed, &mut out).unwrap();
        assert_eq!(consumed, named.len(), "must stop at the compound's end");
    }

    #[test]
    fn map_chunk_body_rewritten_for_764() {
        // Build a map_chunk-shaped body: x, z, a named heightmaps NBT, then a
        // trailing byte standing in for the chunk payload.
        let heightmaps = cubeplane_nbt::Nbt::compound()
            .put_long_array("MOTION_BLOCKING", vec![1, 2])
            .to_bytes_named("");
        let mut body = Vec::new();
        body.extend_from_slice(&1i32.to_be_bytes()); // x
        body.extend_from_slice(&2i32.to_be_bytes()); // z
        body.extend_from_slice(&heightmaps);
        body.push(0xff); // trailing chunk data
        let out = rewrite_clientbound_body(0x24, PROTO_1_20_2, &body).unwrap();
        // x/z preserved, the NBT is now anonymous (one byte shorter: dropped the
        // empty name's 2-byte length), trailing byte preserved.
        assert_eq!(&out[0..8], &body[0..8]);
        assert_eq!(out[8], 0x0a); // compound tag, no name length follows
        assert_eq!(*out.last().unwrap(), 0xff);
        assert_eq!(out.len(), body.len() - 2); // the u16 empty-name length removed
        // Non-1.20.2 protocols leave the body untouched.
        assert!(rewrite_clientbound_body(0x24, 700, &body).is_none());
    }

    #[test]
    fn nbt_empty_tag_end_passthrough() {
        let mut out = Vec::new();
        assert_eq!(named_root_nbt_to_anonymous(&[0x00], &mut out), Some(1));
        assert_eq!(out, vec![0x00]);
    }

    #[test]
    fn maps_are_injective() {
        // No two source ids map to the same target (would corrupt the stream).
        for map in [CB_763_TO_764, SB_764_TO_763] {
            let mut targets: Vec<i32> = map.iter().map(|(_, t)| *t).collect();
            targets.sort_unstable();
            let n = targets.len();
            targets.dedup();
            assert_eq!(targets.len(), n, "duplicate target id in map");
        }
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
