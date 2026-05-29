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
    match protocol {
        PROTO_1_20_2 => rewrite_clientbound_body_764(canonical_id, body),
        // 1.20.3/4 shares 1.20.2's body layouts plus the JSON→NBT text migration.
        PROTO_1_20_3 => {
            // system_chat (763 0x64): content becomes a network NBT text component.
            if canonical_id == 0x64 {
                use cubeplane_protocol::ProtoRead;
                let mut r = BytesMut::from(body);
                let json = r.read_string().ok()?;
                let mut out = chat_json_to_anonymous_nbt(&json)?;
                out.extend_from_slice(&r); // trailing isActionBar bool
                return Some(out);
            }
            rewrite_clientbound_body_764(canonical_id, body)
        }
        // 1.19.4 and 1.19.3 share the same body rewrites; 1.19.3 adds one more.
        PROTO_1_19_4 => rewrite_clientbound_body_119x(canonical_id, body),
        PROTO_1_19_3 => {
            // sync player position (763 0x3c): 1.19.3 appends a dismountVehicle bool.
            if canonical_id == 0x3c {
                let mut out = Vec::with_capacity(body.len() + 1);
                out.extend_from_slice(body);
                out.push(0x00); // dismountVehicle = false
                return Some(out);
            }
            rewrite_clientbound_body_119x(canonical_id, body)
        }
        _ => None,
    }
}

/// Body rewrites for clientbound play packets, 763 → 1.20.2.
fn rewrite_clientbound_body_764(canonical_id: i32, body: &[u8]) -> Option<Vec<u8>> {
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

/// Body rewrites shared by the 1.19.x line (1.19.3/1.19.4). The login layout and
/// the chunk/light `trustEdges` additions are common to both.
fn rewrite_clientbound_body_119x(canonical_id: i32, body: &[u8]) -> Option<Vec<u8>> {
    match canonical_id {
        // login / Join Game (0x28): 1.19.x lacks the trailing portalCooldown.
        0x28 => rewrite_login_763_to_762(body),
        // update_light (0x25): 1.19.4 has a trustEdges bool after chunkX/chunkZ.
        0x25 => {
            let cz_end = varint_field_end(body, varint_field_end(body, 0)?)?;
            let mut out = Vec::with_capacity(body.len() + 1);
            out.extend_from_slice(&body[..cz_end]);
            out.push(0x01); // trustEdges = true
            out.extend_from_slice(&body[cz_end..]);
            Some(out)
        }
        // map_chunk (0x24): 1.19.4 has a trustEdges bool after the blockEntities
        // array (i.e. after x, z, heightmaps NBT, chunkData buffer, blockEntities).
        0x24 => {
            let pos = map_chunk_block_entities_end(body)?;
            let mut out = Vec::with_capacity(body.len() + 1);
            out.extend_from_slice(&body[..pos]);
            out.push(0x01); // trustEdges = true
            out.extend_from_slice(&body[pos..]);
            Some(out)
        }
        _ => None,
    }
}

/// Byte offset just past a varint starting at `at` (update_light's chunkX/chunkZ
/// are varints in 763).
fn varint_field_end(buf: &[u8], at: usize) -> Option<usize> {
    let mut i = at;
    loop {
        let b = *buf.get(i)?;
        i += 1;
        if b & 0x80 == 0 {
            return Some(i);
        }
    }
}

/// Byte offset just past the blockEntities array in a 763 map_chunk body.
fn map_chunk_block_entities_end(body: &[u8]) -> Option<usize> {
    use cubeplane_protocol::ProtoRead;
    let mut r = BytesMut::from(body);
    let total = r.len();
    let _x = r.read_i32().ok()?;
    let _z = r.read_i32().ok()?;
    let hm = named_root_nbt_to_anonymous(&r, &mut Vec::new())?;
    let _ = r.split_to(hm);
    let chunk_data_len = r.read_varint().ok()? as usize;
    let _ = r.split_to(chunk_data_len);
    let count = r.read_varint().ok()?;
    for _ in 0..count {
        // chunkBlockEntity: packedXZ u8, y i16, type varint, nbt (anonymousNbt).
        let _ = r.read_u8().ok()?;
        let _ = r.read_i16().ok()?;
        let _ = r.read_varint().ok()?;
        let n = named_root_nbt_to_anonymous(&r, &mut Vec::new())?;
        let _ = r.split_to(n);
    }
    Some(total - r.len())
}

/// Rewrite the 763 Join Game body into the 1.20.2 layout: the dimension codec is
/// removed (it ships in the Configuration phase), `doLimitedCrafting` is added,
/// and several fields are reordered.
/// The fields of a canonical 763 Join Game packet, including the raw codec bytes
/// (so downgrade targets that keep the inline codec can re-emit it).
struct ParsedLogin {
    entity_id: i32,
    hardcore: bool,
    game_mode: u8,
    prev_game_mode: i8,
    worlds: Vec<String>,
    codec: Vec<u8>,
    world_type: String,
    world_name: String,
    hashed_seed: i64,
    max_players: i32,
    view_distance: i32,
    sim_distance: i32,
    reduced_debug: bool,
    respawn_screen: bool,
    is_debug: bool,
    is_flat: bool,
    has_death: bool,
    portal_cooldown: i32,
}

fn parse_763_login(body: &[u8]) -> Option<ParsedLogin> {
    use cubeplane_protocol::ProtoRead;
    let mut r = BytesMut::from(body);
    let entity_id = r.read_i32().ok()?;
    let hardcore = r.read_bool().ok()?;
    let game_mode = r.read_u8().ok()?;
    let prev_game_mode = r.read_i8().ok()?;
    let world_count = r.read_varint().ok()?;
    let mut worlds = Vec::new();
    for _ in 0..world_count {
        worlds.push(r.read_string().ok()?);
    }
    // The inline dimension codec is a named root NBT compound; capture its bytes.
    let codec_len = named_root_nbt_to_anonymous(&r, &mut Vec::new())?;
    let codec = r.split_to(codec_len).to_vec();
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
    let portal_cooldown = r.read_varint().ok()?;
    Some(ParsedLogin {
        entity_id, hardcore, game_mode, prev_game_mode, worlds, codec, world_type,
        world_name, hashed_seed, max_players, view_distance, sim_distance,
        reduced_debug, respawn_screen, is_debug, is_flat, has_death, portal_cooldown,
    })
}

/// 763 → 1.20.2: reorder fields, drop the inline codec (it ships in the
/// Configuration phase), add `doLimitedCrafting`.
fn rewrite_login_763_to_764(body: &[u8]) -> Option<Vec<u8>> {
    use cubeplane_protocol::ProtoWrite;
    let p = parse_763_login(body)?;
    let mut o = BytesMut::new();
    o.write_i32(p.entity_id);
    o.write_bool(p.hardcore);
    o.write_varint(p.worlds.len() as i32);
    for w in &p.worlds {
        o.write_string(w);
    }
    o.write_varint(p.max_players);
    o.write_varint(p.view_distance);
    o.write_varint(p.sim_distance);
    o.write_bool(p.reduced_debug);
    o.write_bool(p.respawn_screen);
    o.write_bool(false); // doLimitedCrafting (new in 1.20.2)
    o.write_string(&p.world_type);
    o.write_string(&p.world_name);
    o.write_i64(p.hashed_seed);
    o.write_u8(p.game_mode);
    o.write_i8(p.prev_game_mode);
    o.write_bool(p.is_debug);
    o.write_bool(p.is_flat);
    o.write_bool(p.has_death);
    o.write_varint(p.portal_cooldown);
    Some(o.to_vec())
}

/// 763 → 1.19.4: identical layout (codec stays inline) but without the trailing
/// `portalCooldown` field, which 1.19.4 does not have.
fn rewrite_login_763_to_762(body: &[u8]) -> Option<Vec<u8>> {
    use cubeplane_protocol::ProtoWrite;
    let p = parse_763_login(body)?;
    let mut o = BytesMut::new();
    o.write_i32(p.entity_id);
    o.write_bool(p.hardcore);
    o.write_u8(p.game_mode);
    o.write_i8(p.prev_game_mode);
    o.write_varint(p.worlds.len() as i32);
    for w in &p.worlds {
        o.write_string(w);
    }
    o.extend_from_slice(&p.codec);
    o.write_string(&p.world_type);
    o.write_string(&p.world_name);
    o.write_i64(p.hashed_seed);
    o.write_varint(p.max_players);
    o.write_varint(p.view_distance);
    o.write_varint(p.sim_distance);
    o.write_bool(p.reduced_debug);
    o.write_bool(p.respawn_screen);
    o.write_bool(p.is_debug);
    o.write_bool(p.is_flat);
    o.write_bool(p.has_death);
    // No portalCooldown in 1.19.4.
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
        PROTO_1_20_3 => apply_map(wire_id, SB_765_TO_763),
        PROTO_1_19_3 => apply_map(wire_id, SB_761_TO_763),
        _ => wire_id,
    }
}

/// Map a canonical (763) clientbound play packet id to the wire id for a target
/// protocol. Unknown/unsupported protocols fall back to identity.
fn remap_play_clientbound(canonical_id: i32, protocol: i32) -> i32 {
    match protocol {
        PROTO_1_20_2 => apply_map(canonical_id, CB_763_TO_764),
        PROTO_1_20_3 => apply_map(canonical_id, CB_763_TO_765),
        PROTO_1_19_3 => apply_map(canonical_id, CB_763_TO_761),
        // 1.19.4 play ids are identical to 763.
        _ => canonical_id,
    }
}

/// Protocol number for Minecraft 1.20.2.
const PROTO_1_20_2: i32 = 764;
/// Protocol number for Minecraft 1.19.4.
const PROTO_1_19_4: i32 = 762;
/// Protocol number for Minecraft 1.19.3.
const PROTO_1_19_3: i32 = 761;
/// Protocol number for Minecraft 1.20.3 / 1.20.4.
const PROTO_1_20_3: i32 = 765;

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

// Verified play packet-id maps between protocol 763 (1.20.1) and 761 (1.19.3),
// generated from PrismarineJS minecraft-data (pc/1.20 and pc/1.19.3).
const CB_763_TO_761: &[(i32, i32)] = &[
    (0x1, 0x0), (0x2, 0x1), (0x3, 0x2), (0x4, 0x3), (0x5, 0x4), (0x6, 0x5), (0x7, 0x6),
    (0x8, 0x7), (0x9, 0x8), (0xa, 0x9), (0xb, 0xa), (0xc, 0xb), (0xe, 0xc), (0xf, 0xd),
    (0x10, 0xe), (0x11, 0xf), (0x12, 0x10), (0x13, 0x11), (0x14, 0x12), (0x15, 0x13),
    (0x16, 0x14), (0x17, 0x15), (0x19, 0x16), (0x1a, 0x17), (0x1b, 0x18), (0x1c, 0x19),
    (0x1d, 0x1a), (0x1e, 0x1b), (0x1f, 0x1c), (0x20, 0x1d), (0x22, 0x1e), (0x23, 0x1f),
    (0x24, 0x20), (0x25, 0x21), (0x26, 0x22), (0x27, 0x23), (0x28, 0x24), (0x29, 0x25),
    (0x2a, 0x26), (0x2b, 0x27), (0x2c, 0x28), (0x2d, 0x29), (0x2e, 0x2a), (0x2f, 0x2b),
    (0x30, 0x2c), (0x31, 0x2d), (0x32, 0x2e), (0x33, 0x2f), (0x34, 0x30), (0x35, 0x31),
    (0x36, 0x32), (0x37, 0x33), (0x38, 0x34), (0x39, 0x35), (0x3a, 0x36), (0x3b, 0x37),
    (0x3c, 0x38), (0x3d, 0x39), (0x3e, 0x3a), (0x3f, 0x3b), (0x40, 0x3c), (0x41, 0x3d),
    (0x42, 0x3e), (0x43, 0x3f), (0x44, 0x40), (0x45, 0x41), (0x46, 0x42), (0x47, 0x43),
    (0x48, 0x44), (0x49, 0x45), (0x4a, 0x46), (0x4b, 0x47), (0x4c, 0x48), (0x4d, 0x49),
    (0x4e, 0x4a), (0x4f, 0x4b), (0x50, 0x4c), (0x51, 0x4d), (0x52, 0x4e), (0x53, 0x4f),
    (0x54, 0x50), (0x55, 0x51), (0x56, 0x52), (0x57, 0x53), (0x58, 0x54), (0x59, 0x55),
    (0x5a, 0x56), (0x5b, 0x57), (0x5c, 0x58), (0x5d, 0x59), (0x5e, 0x5a), (0x5f, 0x5b),
    (0x60, 0x5c), (0x61, 0x5d), (0x62, 0x5e), (0x63, 0x5f), (0x64, 0x60), (0x65, 0x61),
    (0x66, 0x62), (0x67, 0x63), (0x68, 0x64), (0x69, 0x65), (0x6a, 0x66), (0x6b, 0x67),
    (0x6c, 0x68), (0x6d, 0x69), (0x6e, 0x6a),
];

const SB_761_TO_763: &[(i32, i32)] = &[
    (0x6, 0x7), (0x7, 0x8), (0x8, 0x9), (0x9, 0xa), (0xa, 0xb), (0xb, 0xc), (0xc, 0xd),
    (0xd, 0xe), (0xe, 0xf), (0xf, 0x10), (0x10, 0x11), (0x11, 0x12), (0x12, 0x13),
    (0x13, 0x14), (0x14, 0x15), (0x15, 0x16), (0x16, 0x17), (0x17, 0x18), (0x18, 0x19),
    (0x19, 0x1a), (0x1a, 0x1b), (0x1b, 0x1c), (0x1c, 0x1d), (0x1d, 0x1e), (0x1e, 0x1f),
    (0x1f, 0x20), (0x20, 0x6),
];

// Verified play packet-id maps between protocol 763 (1.20.1) and 765 (1.20.3/4),
// generated from PrismarineJS minecraft-data (pc/1.20 and pc/1.20.3).
const CB_763_TO_765: &[(i32, i32)] = &[
    (0x4, 0x3), (0x5, 0x4), (0x6, 0x5), (0x7, 0x6), (0x8, 0x7), (0x9, 0x8), (0xa, 0x9),
    (0xb, 0xa), (0xc, 0xb), (0xd, 0xe), (0xe, 0xf), (0xf, 0x10), (0x10, 0x11), (0x11, 0x12),
    (0x12, 0x13), (0x13, 0x14), (0x14, 0x15), (0x15, 0x16), (0x16, 0x17), (0x17, 0x18),
    (0x18, 0x19), (0x19, 0x1a), (0x1a, 0x1b), (0x1b, 0x1c), (0x1c, 0x1d), (0x1d, 0x1e),
    (0x1e, 0x1f), (0x1f, 0x20), (0x20, 0x21), (0x21, 0x22), (0x22, 0x23), (0x23, 0x24),
    (0x24, 0x25), (0x25, 0x26), (0x26, 0x27), (0x27, 0x28), (0x28, 0x29), (0x29, 0x2a),
    (0x2a, 0x2b), (0x2b, 0x2c), (0x2c, 0x2d), (0x2d, 0x2e), (0x2e, 0x2f), (0x2f, 0x30),
    (0x30, 0x31), (0x31, 0x32), (0x32, 0x33), (0x33, 0x35), (0x34, 0x36), (0x35, 0x37),
    (0x36, 0x38), (0x37, 0x39), (0x38, 0x3a), (0x39, 0x3b), (0x3a, 0x3c), (0x3b, 0x3d),
    (0x3c, 0x3e), (0x3d, 0x3f), (0x3e, 0x40), (0x3f, 0x41), (0x41, 0x45), (0x42, 0x46),
    (0x43, 0x47), (0x44, 0x48), (0x45, 0x49), (0x46, 0x4a), (0x47, 0x4b), (0x48, 0x4c),
    (0x49, 0x4d), (0x4a, 0x4e), (0x4b, 0x4f), (0x4c, 0x50), (0x4d, 0x51), (0x4e, 0x52),
    (0x4f, 0x53), (0x50, 0x54), (0x51, 0x55), (0x52, 0x56), (0x53, 0x57), (0x54, 0x58),
    (0x55, 0x59), (0x56, 0x5a), (0x57, 0x5b), (0x58, 0x5c), (0x59, 0x5d), (0x5a, 0x5e),
    (0x5b, 0x5f), (0x5c, 0x60), (0x5d, 0x61), (0x5e, 0x62), (0x5f, 0x63), (0x60, 0x64),
    (0x61, 0x65), (0x62, 0x66), (0x63, 0x68), (0x64, 0x69), (0x65, 0x6a), (0x66, 0x6b),
    (0x67, 0x6c), (0x68, 0x6d), (0x69, 0x70), (0x6a, 0x71), (0x6c, 0x72), (0x6d, 0x73),
    (0x6e, 0x74),
];

const SB_765_TO_763: &[(i32, i32)] = &[
    (0x8, 0x7), (0x9, 0x8), (0xa, 0x9), (0xc, 0xa), (0xd, 0xb), (0xe, 0xc), (0x10, 0xd),
    (0x11, 0xe), (0x12, 0xf), (0x13, 0x10), (0x14, 0x11), (0x15, 0x12), (0x16, 0x13),
    (0x17, 0x14), (0x18, 0x15), (0x19, 0x16), (0x1a, 0x17), (0x1b, 0x18), (0x1c, 0x19),
    (0x1d, 0x1a), (0x1f, 0x1b), (0x20, 0x1c), (0x21, 0x1d), (0x22, 0x1e), (0x23, 0x1f),
    (0x24, 0x20), (0x25, 0x21), (0x26, 0x22), (0x27, 0x23), (0x28, 0x24), (0x29, 0x25),
    (0x2a, 0x26), (0x2b, 0x27), (0x2c, 0x28), (0x2d, 0x29), (0x2e, 0x2a), (0x2f, 0x2b),
    (0x30, 0x2c), (0x31, 0x2d), (0x32, 0x2e), (0x33, 0x2f), (0x34, 0x30), (0x35, 0x31),
    (0x36, 0x32),
];

/// Convert a JSON chat component string (763 wire form) into the nameless network
/// NBT text component 1.20.3+ uses. Our server only emits chat as JSON objects,
/// so this handles object/array/scalar nodes recursively. Returns the anonymous
/// NBT bytes.
fn chat_json_to_anonymous_nbt(json: &str) -> Option<Vec<u8>> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let obj = v.as_object()?;
    let mut nbt = cubeplane_nbt::Nbt::compound();
    for (k, val) in obj {
        if let Some(nv) = json_value_to_nbt(val) {
            nbt = nbt.put(k, nv);
        }
    }
    let named = nbt.to_bytes_named(""); // 0x0a 00 00 <payload>
    let mut out = Vec::with_capacity(named.len());
    out.push(0x0a);
    out.extend_from_slice(&named[3..]); // strip the empty name → anonymous
    Some(out)
}

/// Recursively convert a serde_json chat node into an NBT value.
fn json_value_to_nbt(v: &serde_json::Value) -> Option<cubeplane_nbt::Value> {
    use cubeplane_nbt::Value as N;
    Some(match v {
        serde_json::Value::Bool(b) => N::Byte(if *b { 1 } else { 0 }),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                N::Int(i as i32)
            } else {
                N::Double(n.as_f64()?)
            }
        }
        serde_json::Value::String(s) => N::String(s.clone()),
        serde_json::Value::Array(a) => N::List(a.iter().filter_map(json_value_to_nbt).collect()),
        serde_json::Value::Object(o) => {
            let mut nbt = cubeplane_nbt::Nbt::compound();
            for (k, val) in o {
                if let Some(nv) = json_value_to_nbt(val) {
                    nbt = nbt.put(k, nv);
                }
            }
            nbt.into_value()
        }
        serde_json::Value::Null => return None,
    })
}

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
    fn json_chat_converts_to_anonymous_nbt() {
        // A typical server chat component round-trips into a nameless compound
        // whose payload parses as valid NBT.
        let out = chat_json_to_anonymous_nbt(r#"{"text":"hi","color":"red"}"#).unwrap();
        assert_eq!(out[0], 0x0a, "anonymous root compound tag");
        // The converted payload is a valid NBT body (parseable by the converter).
        let consumed = named_root_nbt_to_anonymous(&out, &mut Vec::new()).unwrap();
        assert_eq!(consumed, out.len());
        // Nested extra lists are handled.
        assert!(chat_json_to_anonymous_nbt(r#"{"text":"","extra":[{"text":"a"}]}"#).is_some());
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
