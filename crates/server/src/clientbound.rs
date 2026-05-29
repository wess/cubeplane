//! Builders for clientbound packets (protocol 763).
//!
//! Each function returns a `BytesMut` payload — the packet id VarInt followed
//! by the body — ready to hand to [`crate::codec::encode_frame`]. Keeping these
//! as small pure functions mirrors the composable style of the rest of cubeplane.

use bytes::BytesMut;
use serde_json::Value as Json;
use uuid::Uuid;

use cubeplane_protocol::ProtoWrite;
use cubeplane_world::chunk::{self, Chunk, LightData};

use crate::ids::{login_cb, play_cb, status_cb};
use crate::item::ItemStack;
use crate::registry;

/// Write an item stack in the `slot` wire format, including a `Damage` tag for
/// damaged tools.
fn write_slot(b: &mut BytesMut, stack: ItemStack) {
    if stack.is_empty() {
        b.write_bool(false);
    } else {
        b.write_bool(true);
        b.write_varint(stack.id);
        b.write_i8(stack.count as i8);
        if stack.damage > 0 {
            let nbt = cubeplane_nbt::Nbt::compound().put_int("Damage", stack.damage as i32);
            b.write_bytes(&nbt.to_bytes_named(""));
        } else {
            b.write_u8(0); // optionalNbt absent = TAG_End
        }
    }
}

/// Start a payload buffer with the given packet id.
fn pkt(id: i32) -> BytesMut {
    let mut buf = BytesMut::new();
    buf.write_varint(id);
    buf
}

// ---------------------------------------------------------------------------
// Status state
// ---------------------------------------------------------------------------

pub fn status_response(json: &str) -> BytesMut {
    let mut b = pkt(status_cb::SERVER_INFO);
    b.write_string(json);
    b
}

pub fn status_pong(payload: i64) -> BytesMut {
    let mut b = pkt(status_cb::PONG);
    b.write_i64(payload);
    b
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
    b.write_varint(registry::DIMENSIONS.len() as i32); // world count
    for name in registry::DIMENSIONS {
        b.write_string(name);
    }
    b.write_bytes(&registry::codec().to_bytes_named(""));
    b.write_string(registry::DIMENSION_TYPE);
    b.write_string(registry::DIMENSION_NAME);
    b.write_i64(0); // hashed seed
    b.write_varint(max_players);
    b.write_varint(view_distance); // view distance
    b.write_varint(view_distance); // simulation distance
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
pub fn chunk_data(c: &Chunk, light: &LightData) -> BytesMut {
    let mut b = pkt(play_cb::CHUNK_DATA);
    b.write_i32(c.cx);
    b.write_i32(c.cz);
    b.write_bytes(&c.heightmaps().to_bytes_named(""));

    let sections = c.encode_sections();
    b.write_varint(sections.len() as i32);
    b.write_bytes(&sections);

    b.write_varint(0); // block entity count

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

pub fn unload_chunk(cx: i32, cz: i32) -> BytesMut {
    let mut b = pkt(play_cb::UNLOAD_CHUNK);
    b.write_i32(cx);
    b.write_i32(cz);
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

/// Spawn a non-player entity (mob, projectile, …). `head_yaw` is the living
/// entity's head yaw; `data` is type-specific spawn data (0 for most mobs).
#[allow(clippy::too_many_arguments)]
pub fn spawn_entity(
    entity_id: i32,
    uuid: Uuid,
    type_id: i32,
    x: f64,
    y: f64,
    z: f64,
    yaw: f32,
    pitch: f32,
    head_yaw: f32,
    data: i32,
    velocity: (i16, i16, i16),
) -> BytesMut {
    let mut b = pkt(play_cb::SPAWN_ENTITY);
    b.write_varint(entity_id);
    b.write_uuid(uuid);
    b.write_varint(type_id);
    b.write_f64(x);
    b.write_f64(y);
    b.write_f64(z);
    b.write_i8(angle_to_byte(pitch));
    b.write_i8(angle_to_byte(yaw));
    b.write_i8(angle_to_byte(head_yaw));
    b.write_varint(data);
    b.write_i16(velocity.0);
    b.write_i16(velocity.1);
    b.write_i16(velocity.2);
    b
}

/// Entity Event (status). 2 = generic hurt, 3 = death animation.
pub fn entity_status(entity_id: i32, status: i8) -> BytesMut {
    let mut b = pkt(play_cb::ENTITY_STATUS);
    b.write_i32(entity_id);
    b.write_i8(status);
    b
}

/// Play the hurt-flash animation on an entity, facing `yaw` (degrees).
pub fn hurt_animation(entity_id: i32, yaw: f32) -> BytesMut {
    let mut b = pkt(play_cb::HURT_ANIMATION);
    b.write_varint(entity_id);
    b.write_f32(yaw);
    b
}

/// Set an entity's velocity (units of 1/8000 block per tick).
pub fn entity_velocity(entity_id: i32, vx: i16, vy: i16, vz: i16) -> BytesMut {
    let mut b = pkt(play_cb::ENTITY_VELOCITY);
    b.write_varint(entity_id);
    b.write_i16(vx);
    b.write_i16(vy);
    b.write_i16(vz);
    b
}

/// Update the player's health, food and saturation HUD.
pub fn update_health(health: f32, food: i32, saturation: f32) -> BytesMut {
    let mut b = pkt(play_cb::UPDATE_HEALTH);
    b.write_f32(health);
    b.write_varint(food);
    b.write_f32(saturation);
    b
}

/// Show the death screen for a player entity with the given message JSON.
pub fn death_combat_event(player_entity_id: i32, message: &Json) -> BytesMut {
    let mut b = pkt(play_cb::DEATH_COMBAT_EVENT);
    b.write_varint(player_entity_id);
    b.write_string(&message.to_string());
    b
}

/// Ask the client to download and apply a server resource pack.
pub fn resource_pack(url: &str, hash: &str, forced: bool) -> BytesMut {
    let mut b = pkt(play_cb::RESOURCE_PACK);
    b.write_string(url);
    b.write_string(hash);
    b.write_bool(forced);
    b.write_bool(false); // no prompt message
    b
}

/// Respawn the player into a (re)loaded world in dimension `dim`.
pub fn respawn(dim: u8, gamemode: u8, is_flat: bool) -> BytesMut {
    let mut b = pkt(play_cb::RESPAWN);
    b.write_string(registry::dim_id(dim));
    b.write_string(registry::dim_id(dim));
    b.write_i64(0); // hashed seed
    b.write_i8(gamemode as i8);
    b.write_u8(255); // previous gamemode = -1 (none)
    b.write_bool(false); // is debug
    b.write_bool(is_flat);
    b.write_bool(false); // copy metadata (false = full reset)
    b.write_bool(false); // has death location
    b.write_varint(0); // portal cooldown
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

/// Initialize the world border (centred at origin) with the given diameter.
pub fn init_world_border(diameter: f64) -> BytesMut {
    let mut b = pkt(play_cb::INITIALIZE_WORLD_BORDER);
    b.write_f64(0.0); // center x
    b.write_f64(0.0); // center z
    b.write_f64(diameter); // old diameter
    b.write_f64(diameter); // new diameter
    b.write_varint(0); // speed (ms to reach new size; 0 = static)
    b.write_varint(29_999_984); // portal teleport boundary
    b.write_varint(5); // warning blocks
    b.write_varint(15); // warning time
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

// ---------------------------------------------------------------------------
// Inventory, items & entities
// ---------------------------------------------------------------------------

/// Set the passengers riding an entity (e.g. a player in a boat).
pub fn set_passengers(vehicle: i32, passengers: &[i32]) -> BytesMut {
    let mut b = pkt(play_cb::SET_PASSENGERS);
    b.write_varint(vehicle);
    b.write_varint(passengers.len() as i32);
    for p in passengers {
        b.write_varint(*p);
    }
    b
}

/// A villager's trade list. Each offer is `(input1, output, input2)`.
pub fn trade_list(window_id: i32, offers: &[(ItemStack, ItemStack, ItemStack)]) -> BytesMut {
    let mut b = pkt(play_cb::TRADE_LIST);
    b.write_varint(window_id);
    b.write_varint(offers.len() as i32);
    for (in1, out, in2) in offers {
        write_slot(&mut b, *in1);
        write_slot(&mut b, *out);
        write_slot(&mut b, *in2);
        b.write_bool(false); // trade disabled
        b.write_i32(0); // uses
        b.write_i32(999); // max uses
        b.write_i32(2); // xp
        b.write_i32(0); // special price
        b.write_f32(0.0); // price multiplier
        b.write_i32(0); // demand
    }
    b.write_varint(1); // villager level
    b.write_varint(0); // experience
    b.write_bool(true); // regular villager
    b.write_bool(false); // can restock
    b
}

/// Open the sign-edit screen for a freshly-placed sign.
pub fn open_sign_editor(x: i32, y: i32, z: i32) -> BytesMut {
    let mut b = pkt(play_cb::OPEN_SIGN_EDITOR);
    b.write_position(x, y, z);
    b.write_bool(true); // front text
    b
}

/// Open a container window. `inv_type` 2 = generic 9×3 (single chest).
pub fn open_window(window_id: i32, inv_type: i32, title: &Json) -> BytesMut {
    let mut b = pkt(play_cb::OPEN_WINDOW);
    b.write_varint(window_id);
    b.write_varint(inv_type);
    b.write_string(&title.to_string());
    b
}

/// Full inventory sync (Set Container Content).
pub fn window_items(window_id: u8, state_id: i32, items: &[ItemStack], carried: ItemStack) -> BytesMut {
    let mut b = pkt(play_cb::WINDOW_ITEMS);
    b.write_u8(window_id);
    b.write_varint(state_id);
    b.write_varint(items.len() as i32);
    for s in items {
        write_slot(&mut b, *s);
    }
    write_slot(&mut b, carried);
    b
}

/// Set a window property (e.g. furnace progress/fuel arrows).
pub fn window_property(window_id: u8, property: i16, value: i16) -> BytesMut {
    let mut b = pkt(play_cb::WINDOW_PROPERTY);
    b.write_u8(window_id);
    b.write_i16(property);
    b.write_i16(value);
    b
}

/// Update a single inventory slot (Set Container Slot).
pub fn set_slot(window_id: i8, state_id: i32, slot: i16, item: ItemStack) -> BytesMut {
    let mut b = pkt(play_cb::SET_SLOT);
    b.write_i8(window_id);
    b.write_varint(state_id);
    b.write_i16(slot);
    write_slot(&mut b, item);
    b
}

/// A single entity-metadata entry (index + typed value).
pub enum Meta {
    Byte(u8, i8),
    VarInt(u8, i32),
    Float(u8, f32),
    Bool(u8, bool),
}

/// Set arbitrary entity metadata (variants, flags, …). Entry types match the
/// 1.20.1 metadata type ids: 0=byte, 1=varint, 3=float, 8=boolean.
pub fn entity_metadata(entity_id: i32, entries: &[Meta]) -> BytesMut {
    let mut b = pkt(play_cb::ENTITY_METADATA);
    b.write_varint(entity_id);
    for e in entries {
        match e {
            Meta::Byte(i, v) => {
                b.write_u8(*i);
                b.write_varint(0);
                b.write_i8(*v);
            }
            Meta::VarInt(i, v) => {
                b.write_u8(*i);
                b.write_varint(1);
                b.write_varint(*v);
            }
            Meta::Float(i, v) => {
                b.write_u8(*i);
                b.write_varint(3);
                b.write_f32(*v);
            }
            Meta::Bool(i, v) => {
                b.write_u8(*i);
                b.write_varint(8);
                b.write_bool(*v);
            }
        }
    }
    b.write_u8(0xff); // end of metadata
    b
}

/// Give an entity a visible custom name (floating nameplate). Uses metadata
/// index 2 (optional chat component) + index 3 (name visible bool).
pub fn entity_custom_name(entity_id: i32, name: &Json) -> BytesMut {
    let mut b = pkt(play_cb::ENTITY_METADATA);
    b.write_varint(entity_id);
    b.write_u8(2); // index 2 = custom name
    b.write_varint(6); // type 6 = optional chat component
    b.write_bool(true); // present
    b.write_string(&name.to_string());
    b.write_u8(3); // index 3 = custom name visible
    b.write_varint(8); // type 8 = boolean
    b.write_bool(true);
    b.write_u8(0xff); // end
    b
}

/// Set the entity's `Item` metadata so an item entity renders its stack.
pub fn entity_metadata_item(entity_id: i32, stack: ItemStack) -> BytesMut {
    let mut b = pkt(play_cb::ENTITY_METADATA);
    b.write_varint(entity_id);
    b.write_u8(8); // metadata index 8 = Item (for item entities)
    b.write_varint(7); // type 7 = item_stack
    write_slot(&mut b, stack);
    b.write_u8(0xff); // end of metadata
    b
}

/// Spawn an experience orb.
pub fn spawn_xp_orb(entity_id: i32, x: f64, y: f64, z: f64, count: i16) -> BytesMut {
    let mut b = pkt(play_cb::SPAWN_XP_ORB);
    b.write_varint(entity_id);
    b.write_f64(x);
    b.write_f64(y);
    b.write_f64(z);
    b.write_i16(count);
    b
}

/// Animate an item entity being picked up.
pub fn collect_item(collected: i32, collector: i32, count: i32) -> BytesMut {
    let mut b = pkt(play_cb::COLLECT);
    b.write_varint(collected);
    b.write_varint(collector);
    b.write_varint(count);
    b
}

/// Update the XP bar/level HUD.
pub fn set_experience(bar: f32, level: i32, total: i32) -> BytesMut {
    let mut b = pkt(play_cb::SET_EXPERIENCE);
    b.write_f32(bar);
    b.write_varint(level);
    b.write_varint(total);
    b
}

/// Apply a status effect to an entity. `flags`: bit0 ambient, bit1 particles.
pub fn entity_effect(entity_id: i32, effect_id: i32, amplifier: i8, duration: i32, flags: i8) -> BytesMut {
    let mut b = pkt(play_cb::ENTITY_EFFECT);
    b.write_varint(entity_id);
    b.write_varint(effect_id);
    b.write_i8(amplifier);
    b.write_varint(duration);
    b.write_i8(flags);
    b.write_bool(false); // no factor codec
    b
}

/// An explosion at `(x,y,z)` destroying the given block offsets.
pub fn explosion(x: f64, y: f64, z: f64, radius: f32, offsets: &[(i8, i8, i8)]) -> BytesMut {
    let mut b = pkt(play_cb::EXPLOSION);
    b.write_f64(x);
    b.write_f64(y);
    b.write_f64(z);
    b.write_f32(radius);
    b.write_varint(offsets.len() as i32);
    for (ox, oy, oz) in offsets {
        b.write_i8(*ox);
        b.write_i8(*oy);
        b.write_i8(*oz);
    }
    b.write_f32(0.0);
    b.write_f32(0.0);
    b.write_f32(0.0);
    b
}

/// Play a sound *by name* at a position (e.g. "entity.player.hurt"). Uses the
/// inline sound-event form, so no numeric registry id is needed. `category` is
/// a soundSource id (7 = player, 6 = neutral, 5 = hostile, 4 = block).
#[allow(clippy::too_many_arguments)]
pub fn sound_effect(name: &str, category: i32, x: f64, y: f64, z: f64, volume: f32, pitch: f32) -> BytesMut {
    let mut b = pkt(play_cb::SOUND_EFFECT);
    b.write_varint(0); // registryEntryHolder: 0 = inline definition
    let ident = if name.contains(':') { name.to_string() } else { format!("minecraft:{name}") };
    b.write_string(&ident);
    b.write_bool(false); // no fixed range
    b.write_varint(category);
    b.write_i32((x * 8.0) as i32);
    b.write_i32((y * 8.0) as i32);
    b.write_i32((z * 8.0) as i32);
    b.write_f32(volume);
    b.write_f32(pitch);
    b.write_i64(0); // seed
    b
}

/// Spawn particles at a point. Exposed for mods/future use.
#[allow(clippy::too_many_arguments, dead_code)]
pub fn particle(particle_id: i32, x: f64, y: f64, z: f64, spread: f32, count: i32) -> BytesMut {
    let mut b = pkt(play_cb::WORLD_PARTICLES);
    b.write_varint(particle_id);
    b.write_bool(true); // long distance
    b.write_f64(x);
    b.write_f64(y);
    b.write_f64(z);
    b.write_f32(spread);
    b.write_f32(spread);
    b.write_f32(spread);
    b.write_f32(0.1); // particle data (speed)
    b.write_i32(count);
    b
}

/// Declare the available commands as a Brigadier graph so the client offers
/// tab-completion and colours known commands. Each entry is `(name, has_args)`;
/// commands with args get a greedy-string argument child so free-text after the
/// command name is accepted.
pub fn declare_commands(commands: &[(&str, bool)]) -> BytesMut {
    // Lay out nodes: 0 = root, then per command a literal node and (if it
    // takes args) an argument node.
    struct Node {
        flags: u8,
        children: Vec<i32>,
        name: Option<String>,
        parser: Option<()>, // present ⇒ brigadier:string (greedy)
    }
    let mut nodes: Vec<Node> = vec![Node { flags: 0x00, children: vec![], name: None, parser: None }];
    let mut root_children = Vec::new();

    for (name, has_args) in commands {
        let arg_index = if *has_args {
            // type=argument(2) | has_command(0x04)
            nodes.push(Node { flags: 0x06, children: vec![], name: Some("args".into()), parser: Some(()) });
            Some(nodes.len() as i32 - 1)
        } else {
            None
        };
        // type=literal(1) | has_command(0x04)
        let children = arg_index.into_iter().collect();
        nodes.push(Node { flags: 0x05, children, name: Some((*name).into()), parser: None });
        root_children.push(nodes.len() as i32 - 1);
    }
    nodes[0].children = root_children;

    let mut b = pkt(play_cb::DECLARE_COMMANDS);
    b.write_varint(nodes.len() as i32);
    for n in &nodes {
        b.write_u8(n.flags);
        b.write_varint(n.children.len() as i32);
        for c in &n.children {
            b.write_varint(*c);
        }
        if let Some(name) = &n.name {
            b.write_string(name);
        }
        if n.parser.is_some() {
            b.write_varint(5); // parser id 5 = brigadier:string
            b.write_varint(2); // string mode 2 = greedy phrase
        }
    }
    b.write_varint(0); // root index
    b
}

/// Show a big title text.
pub fn title_text(text: &Json) -> BytesMut {
    let mut b = pkt(play_cb::SET_TITLE_TEXT);
    b.write_string(&text.to_string());
    b
}

/// Show subtitle text (paired with [`title_text`]).
pub fn title_subtitle(text: &Json) -> BytesMut {
    let mut b = pkt(play_cb::SET_TITLE_SUBTITLE);
    b.write_string(&text.to_string());
    b
}

/// Set title fade-in / stay / fade-out times (ticks).
pub fn title_times(fade_in: i32, stay: i32, fade_out: i32) -> BytesMut {
    let mut b = pkt(play_cb::SET_TITLE_TIME);
    b.write_i32(fade_in);
    b.write_i32(stay);
    b.write_i32(fade_out);
    b
}

/// Set the tab list header and footer (JSON text components).
pub fn tab_list_header(header: &Json, footer: &Json) -> BytesMut {
    let mut b = pkt(play_cb::TAB_LIST_HEADER);
    b.write_string(&header.to_string());
    b.write_string(&footer.to_string());
    b
}

/// Convert a degrees angle to the 1/256-of-a-turn byte the protocol uses.
pub fn angle_to_byte(deg: f32) -> i8 {
    (((deg.rem_euclid(360.0)) / 360.0 * 256.0).round() as i64 & 0xFF) as i8
}
