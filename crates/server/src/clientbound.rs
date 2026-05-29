//! Builders for clientbound packets (protocol 763).
//!
//! Each function returns a `BytesMut` payload — the packet id VarInt followed
//! by the body — ready to hand to [`crate::codec::encode_frame`]. Keeping these
//! as small pure functions mirrors the composable style of the rest of cubeplane.

use bytes::BytesMut;
use serde_json::Value as Json;
use uuid::Uuid;

use cubeplane_protocol::ProtoWrite;
use cubeplane_world::chunk::{self, Chunk};

use crate::ids::{login_cb, play_cb};
use crate::registry;

/// Start a payload buffer with the given packet id.
fn pkt(id: i32) -> BytesMut {
    let mut buf = BytesMut::new();
    buf.write_varint(id);
    buf
}

// ---------------------------------------------------------------------------
// Login state
// ---------------------------------------------------------------------------

pub fn login_success(uuid: Uuid, name: &str) -> BytesMut {
    let mut b = pkt(login_cb::SUCCESS);
    b.write_uuid(uuid);
    b.write_string(name);
    b.write_varint(0); // number of properties
    b
}

pub fn set_compression(threshold: i32) -> BytesMut {
    let mut b = pkt(login_cb::SET_COMPRESSION);
    b.write_varint(threshold);
    b
}

pub fn login_disconnect(reason: &Json) -> BytesMut {
    let mut b = pkt(login_cb::DISCONNECT);
    b.write_string(&reason.to_string());
    b
}

// ---------------------------------------------------------------------------
// Play state
// ---------------------------------------------------------------------------

/// Login (Play) — "Join Game". Establishes dimension, registries and rules.
#[allow(clippy::too_many_arguments)]
pub fn join_game(
    entity_id: i32,
    gamemode: u8,
    max_players: i32,
    view_distance: i32,
    is_flat: bool,
) -> BytesMut {
    let mut b = pkt(play_cb::LOGIN);
    b.write_i32(entity_id);
    b.write_bool(false); // isHardcore
    b.write_u8(gamemode);
    b.write_i8(-1); // previousGameMode = none
    b.write_varint(1); // world count
    b.write_string(registry::DIMENSION_NAME);
    b.write_bytes(&registry::codec().to_bytes_named(""));
    b.write_string(registry::DIMENSION_TYPE);
    b.write_string(registry::DIMENSION_NAME);
    b.write_i64(0); // hashed seed
    b.write_varint(max_players);
    b.write_varint(view_distance);
    b.write_varint(view_distance.min(view_distance)); // simulation distance
    b.write_bool(false); // reduced debug info
    b.write_bool(true); // enable respawn screen
    b.write_bool(false); // is debug
    b.write_bool(is_flat);
    b.write_bool(false); // has death location
    b.write_varint(0); // portal cooldown
    b
}

pub fn spawn_position(x: i32, y: i32, z: i32, angle: f32) -> BytesMut {
    let mut b = pkt(play_cb::SPAWN_POSITION);
    b.write_position(x, y, z);
    b.write_f32(angle);
    b
}

/// `flags` is a bitmask: 0x01 invulnerable, 0x02 flying, 0x04 allow flying,
/// 0x08 creative instant break.
pub fn player_abilities(flags: i8, flying_speed: f32, walk_speed: f32) -> BytesMut {
    let mut b = pkt(play_cb::PLAYER_ABILITIES);
    b.write_i8(flags);
    b.write_f32(flying_speed);
    b.write_f32(walk_speed);
    b
}

#[allow(clippy::too_many_arguments)]
pub fn sync_position(
    x: f64,
    y: f64,
    z: f64,
    yaw: f32,
    pitch: f32,
    flags: i8,
    teleport_id: i32,
) -> BytesMut {
    let mut b = pkt(play_cb::SYNC_POSITION);
    b.write_f64(x);
    b.write_f64(y);
    b.write_f64(z);
    b.write_f32(yaw);
    b.write_f32(pitch);
    b.write_i8(flags);
    b.write_varint(teleport_id);
    b
}

pub fn set_center_chunk(cx: i32, cz: i32) -> BytesMut {
    let mut b = pkt(play_cb::SET_CENTER_CHUNK);
    b.write_varint(cx);
    b.write_varint(cz);
    b
}

/// Chunk Data and Update Light. Encapsulates sections, heightmaps and full
/// skylight so the client renders a lit, walkable column.
pub fn chunk_data(c: &Chunk) -> BytesMut {
    let mut b = pkt(play_cb::CHUNK_DATA);
    b.write_i32(c.cx);
    b.write_i32(c.cz);
    b.write_bytes(&c.heightmaps().to_bytes_named(""));

    let sections = c.encode_sections();
    b.write_varint(sections.len() as i32);
    b.write_bytes(&sections);

    b.write_varint(0); // block entity count

    let light = c.full_sky_light();
    chunk::write_bitset(&mut b, light.sky_light_mask);
    chunk::write_bitset(&mut b, light.block_light_mask);
    chunk::write_bitset(&mut b, light.empty_sky_light_mask);
    chunk::write_bitset(&mut b, light.empty_block_light_mask);

    b.write_varint(light.sky_light.len() as i32);
    for array in &light.sky_light {
        b.write_varint(array.len() as i32);
        b.write_bytes(array);
    }
    b.write_varint(light.block_light.len() as i32);
    for array in &light.block_light {
        b.write_varint(array.len() as i32);
        b.write_bytes(array);
    }
    b
}

pub fn keep_alive(id: i64) -> BytesMut {
    let mut b = pkt(play_cb::KEEP_ALIVE);
    b.write_i64(id);
    b
}

pub fn system_chat(content: &Json, action_bar: bool) -> BytesMut {
    let mut b = pkt(play_cb::SYSTEM_CHAT);
    b.write_string(&content.to_string());
    b.write_bool(action_bar);
    b
}

/// A single entry for [`player_info_add`].
pub struct PlayerListEntry {
    pub uuid: Uuid,
    pub name: String,
    pub gamemode: i32,
    pub latency: i32,
}

/// Player Info Update with add_player + game_mode + listed + latency actions.
pub fn player_info_add(entries: &[PlayerListEntry]) -> BytesMut {
    const ADD_PLAYER: u8 = 0x01;
    const UPDATE_GAME_MODE: u8 = 0x04;
    const UPDATE_LISTED: u8 = 0x08;
    const UPDATE_LATENCY: u8 = 0x10;

    let mut b = pkt(play_cb::PLAYER_INFO_UPDATE);
    b.write_u8(ADD_PLAYER | UPDATE_GAME_MODE | UPDATE_LISTED | UPDATE_LATENCY);
    b.write_varint(entries.len() as i32);
    for e in entries {
        b.write_uuid(e.uuid);
        // add_player: game profile
        b.write_string(&e.name);
        b.write_varint(0); // no properties (skin)
        // update_game_mode
        b.write_varint(e.gamemode);
        // update_listed
        b.write_bool(true);
        // update_latency
        b.write_varint(e.latency);
    }
    b
}

pub fn player_info_remove(uuids: &[Uuid]) -> BytesMut {
    let mut b = pkt(play_cb::PLAYER_INFO_REMOVE);
    b.write_varint(uuids.len() as i32);
    for u in uuids {
        b.write_uuid(*u);
    }
    b
}

#[allow(clippy::too_many_arguments)]
pub fn spawn_player(entity_id: i32, uuid: Uuid, x: f64, y: f64, z: f64, yaw: f32, pitch: f32) -> BytesMut {
    let mut b = pkt(play_cb::SPAWN_PLAYER);
    b.write_varint(entity_id);
    b.write_uuid(uuid);
    b.write_f64(x);
    b.write_f64(y);
    b.write_f64(z);
    b.write_i8(angle_to_byte(yaw));
    b.write_i8(angle_to_byte(pitch));
    b
}

pub fn remove_entities(ids: &[i32]) -> BytesMut {
    let mut b = pkt(play_cb::REMOVE_ENTITIES);
    b.write_varint(ids.len() as i32);
    for id in ids {
        b.write_varint(*id);
    }
    b
}

/// Absolute reposition of a remote entity. Used for other players' movement —
/// simpler and overflow-free compared to the relative-move packets.
pub fn entity_teleport(entity_id: i32, x: f64, y: f64, z: f64, yaw: f32, pitch: f32, on_ground: bool) -> BytesMut {
    let mut b = pkt(play_cb::ENTITY_TELEPORT);
    b.write_varint(entity_id);
    b.write_f64(x);
    b.write_f64(y);
    b.write_f64(z);
    b.write_i8(angle_to_byte(yaw));
    b.write_i8(angle_to_byte(pitch));
    b.write_bool(on_ground);
    b
}

pub fn entity_head_rotation(entity_id: i32, yaw: f32) -> BytesMut {
    let mut b = pkt(play_cb::ENTITY_HEAD_ROTATION);
    b.write_varint(entity_id);
    b.write_i8(angle_to_byte(yaw));
    b
}

pub fn block_update(x: i32, y: i32, z: i32, state: u16) -> BytesMut {
    let mut b = pkt(play_cb::BLOCK_UPDATE);
    b.write_position(x, y, z);
    b.write_varint(state as i32);
    b
}

pub fn acknowledge_block_change(sequence: i32) -> BytesMut {
    let mut b = pkt(play_cb::ACKNOWLEDGE_BLOCK_CHANGE);
    b.write_varint(sequence);
    b
}

pub fn set_held_item(slot: u8) -> BytesMut {
    let mut b = pkt(play_cb::SET_HELD_ITEM);
    b.write_u8(slot);
    b
}

/// Game Event. `reason` 13 = "start waiting for chunks" / level chunks loaded.
pub fn game_event(reason: u8, value: f32) -> BytesMut {
    let mut b = pkt(play_cb::GAME_EVENT);
    b.write_u8(reason);
    b.write_f32(value);
    b
}

pub fn update_time(world_age: i64, time_of_day: i64) -> BytesMut {
    let mut b = pkt(play_cb::UPDATE_TIME);
    b.write_i64(world_age);
    b.write_i64(time_of_day);
    b
}

pub fn play_disconnect(reason: &Json) -> BytesMut {
    let mut b = pkt(play_cb::DISCONNECT);
    b.write_string(&reason.to_string());
    b
}

/// Convert a degrees angle to the 1/256-of-a-turn byte the protocol uses.
pub fn angle_to_byte(deg: f32) -> i8 {
    (((deg.rem_euclid(360.0)) / 360.0 * 256.0).round() as i64 & 0xFF) as i8
}
