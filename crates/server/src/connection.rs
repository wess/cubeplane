//! The per-connection lifecycle: handshake → status/login → play.

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::Result;
use serde_json::json;
use tokio::io::BufReader;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpStream;
use tokio::sync::mpsc::unbounded_channel;
use tracing::{debug, info};

use cubeplane_mods::ModEvent;
use cubeplane_protocol::{ProtoRead, RawPacket, PROTOCOL_VERSION};

use crate::clientbound as cb;
use crate::codec::{read_frame, write_frame, NO_COMPRESSION};
use crate::combat;
use crate::commands;
use crate::drops;
use crate::item;
use crate::mobs;
use crate::ids::{login_sb, status_sb};
use crate::player::{offline_uuid, Player};
use crate::serverbound::{self, Play};
use crate::state::Shared;
use crate::text;

type Reader = BufReader<OwnedReadHalf>;

/// Entry point for a freshly-accepted TCP connection.
pub async fn handle(stream: TcpStream, shared: Arc<Shared>) {
    let peer = stream.peer_addr().ok();
    stream.set_nodelay(true).ok();
    if let Err(e) = drive(stream, shared).await {
        debug!(?peer, "connection closed: {e}");
    }
}

async fn drive(stream: TcpStream, shared: Arc<Shared>) -> Result<()> {
    let (rh, wh) = stream.into_split();
    let mut reader = BufReader::new(rh);
    let mut writer = wh;

    // --- Handshake -----------------------------------------------------------
    let frame = read_frame(&mut reader, NO_COMPRESSION).await?;
    let mut raw = RawPacket::parse(frame)?;
    let _protocol = raw.body.read_varint()?;
    let _host = raw.body.read_string()?;
    let _port = raw.body.read_u16()?;
    let next_state = raw.body.read_varint()?;

    match next_state {
        1 => status(&mut reader, &mut writer, &shared).await,
        2 => login(reader, writer, shared).await,
        other => {
            debug!("unknown next-state {other} in handshake");
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Status (server list ping)
// ---------------------------------------------------------------------------

async fn status(reader: &mut Reader, writer: &mut OwnedWriteHalf, shared: &Arc<Shared>) -> Result<()> {
    loop {
        let frame = read_frame(reader, NO_COMPRESSION).await?;
        if frame.is_empty() {
            continue;
        }
        let mut raw = RawPacket::parse(frame)?;
        match raw.id {
            status_sb::REQUEST => {
                let payload = cb::status_response(&status_json(shared));
                write_frame(writer, &payload, NO_COMPRESSION).await?;
            }
            status_sb::PING => {
                let token = raw.body.read_i64().unwrap_or(0);
                let payload = cb::status_pong(token);
                write_frame(writer, &payload, NO_COMPRESSION).await?;
                return Ok(());
            }
            _ => return Ok(()),
        }
    }
}

fn status_json(shared: &Arc<Shared>) -> String {
    json!({
        "version": { "name": cubeplane_protocol::GAME_VERSION, "protocol": PROTOCOL_VERSION },
        "players": {
            "max": shared.config.server.max_players,
            "online": shared.player_count(),
            "sample": []
        },
        "description": { "text": shared.config.server.motd }
    })
    .to_string()
}

// ---------------------------------------------------------------------------
// Login
// ---------------------------------------------------------------------------

async fn login(mut reader: Reader, mut writer: OwnedWriteHalf, shared: Arc<Shared>) -> Result<()> {
    let frame = read_frame(&mut reader, NO_COMPRESSION).await?;
    let mut raw = RawPacket::parse(frame)?;
    if raw.id != login_sb::LOGIN_START {
        return Ok(());
    }
    let name = raw.body.read_string()?;
    let uuid = offline_uuid(&name);

    // Reject if full.
    if shared.player_count() as i32 >= shared.config.server.max_players {
        let reason = text::colored("cubeplane is full", "red");
        write_frame(&mut writer, &cb::login_disconnect(&reason), NO_COMPRESSION).await?;
        return Ok(());
    }

    // Negotiate compression, then confirm the login.
    let threshold = shared.config.server.compression_threshold;
    if threshold >= 0 {
        write_frame(&mut writer, &cb::set_compression(threshold), NO_COMPRESSION).await?;
    }
    write_frame(&mut writer, &cb::login_success(uuid, &name), threshold).await?;

    info!(%name, %uuid, "player logging in");
    play(reader, writer, shared, name, uuid, threshold).await
}

// ---------------------------------------------------------------------------
// Play
// ---------------------------------------------------------------------------

async fn play(
    mut reader: Reader,
    writer: OwnedWriteHalf,
    shared: Arc<Shared>,
    name: String,
    uuid: uuid::Uuid,
    threshold: i32,
) -> Result<()> {
    let entity_id = shared.next_entity_id();
    let gamemode = shared.config.gamemode_id() as i32;
    let spawn = { shared.world.lock().unwrap().spawn() };

    // Outgoing packet channel feeding a dedicated writer task.
    let (tx, mut rx) = unbounded_channel::<bytes::BytesMut>();
    let writer_task = tokio::spawn(async move {
        let mut writer = writer;
        while let Some(payload) = rx.recv().await {
            if write_frame(&mut writer, &payload, threshold).await.is_err() {
                break;
            }
        }
    });

    let player = Player::new(entity_id, uuid, name.clone(), gamemode, tx, spawn);

    // Restore saved player data (position, vitals, inventory, XP) if present.
    let save_dir = std::path::PathBuf::from(&shared.config.world.save_dir);
    if shared.config.world.save {
        if let Some(data) = crate::persistence::load_player(&save_dir, uuid) {
            player.apply_data(&data);
        }
    }
    let spawn = {
        let s = player.state();
        (s.x, s.y, s.z)
    };

    // --- Join sequence -------------------------------------------------------
    let is_flat = shared.world.lock().unwrap().generator_name() == "flat";
    player.send(cb::join_game(
        entity_id,
        gamemode as u8,
        shared.config.server.max_players,
        shared.config.server.view_distance,
        is_flat,
    ));

    let abilities = if shared.config.is_creative() { 0x0D } else { 0x00 };
    player.send(cb::player_abilities(abilities, 0.05, 0.1));
    player.send(cb::set_held_item(0));
    player.send(cb::spawn_position(spawn.0 as i32, spawn.1 as i32, spawn.2 as i32, 0.0));
    player.send(cb::sync_position(spawn.0, spawn.1, spawn.2, 0.0, 0.0, 0, 0));
    {
        let s = player.state();
        player.send(cb::update_health(s.health, s.food, s.saturation));
        player.send(cb::set_experience(combat::xp_bar(s.xp_total), s.xp_total / 10, s.xp_total));
    }
    player.sync_inventory();
    player.send(cb::declare_commands(&[
        "help", "list", "pos", "tp", "gamemode", "give", "time", "weather", "summon", "effect",
        "heal", "kill", "clear", "xp",
    ]));
    player.send(cb::tab_list_header(
        &text::colored("cubeplane", "gold"),
        &text::colored("Rust engine · JS mods", "gray"),
    ));
    // "Start waiting for level chunks" so the client renders the world.
    player.send(cb::game_event(13, 0.0));

    // Stream the spawn-area chunks.
    let mut loaded: HashSet<(i32, i32)> = HashSet::new();
    let (cx, cz) = (
        (spawn.0.floor() as i32).div_euclid(16),
        (spawn.2.floor() as i32).div_euclid(16),
    );
    player.send(cb::set_center_chunk(cx, cz));
    stream_chunks(&shared, &player, cx, cz, &mut loaded);

    // --- Player list & entity spawns ----------------------------------------
    let existing = shared.players();
    shared.add_player(player.clone());

    // Tell the newcomer about everyone (including themselves).
    let mut entries: Vec<cb::PlayerListEntry> = existing.iter().map(list_entry).collect();
    entries.push(list_entry(&player));
    player.send(cb::player_info_add(&entries));

    // Tell everyone else about the newcomer.
    shared.broadcast_except(entity_id, cb::player_info_add(&[list_entry(&player)]));

    // Spawn existing players for the newcomer, and the newcomer for everyone.
    for p in &existing {
        let s = p.state();
        player.send(cb::spawn_player(p.entity_id, p.uuid, s.x, s.y, s.z, s.yaw, s.pitch));
        player.send(cb::entity_head_rotation(p.entity_id, s.yaw));
    }
    shared.broadcast_except(
        entity_id,
        cb::spawn_player(entity_id, uuid, spawn.0, spawn.1, spawn.2, 0.0, 0.0),
    );

    // Show the newcomer every mob already roaming the world.
    for m in shared.mobs() {
        player.send(cb::spawn_entity(
            m.entity_id, m.uuid, m.kind.type_id(), m.x, m.y, m.z, m.yaw, m.pitch, m.yaw, 0, (0, 0, 0),
        ));
    }

    // Announce + notify mods.
    let join_msg = text::system_notice(format!("{name} joined the cubeplane"));
    shared.broadcast(cb::system_chat(&join_msg, false));
    info!(%name, players = shared.player_count(), "player joined");
    shared.fire_mod(ModEvent::PlayerJoin {
        player: name.clone(),
        uuid: uuid.to_string(),
        entity_id,
    });

    // --- Main packet loop ----------------------------------------------------
    let mut last_center = (cx, cz);
    let result = play_loop(&mut reader, &shared, &player, threshold, &mut loaded, &mut last_center).await;

    // --- Cleanup -------------------------------------------------------------
    if shared.config.world.save {
        let _ = crate::persistence::save_player(&save_dir, uuid, &player.snapshot_data());
    }
    shared.remove_player(entity_id);
    shared.broadcast(cb::player_info_remove(&[uuid]));
    shared.broadcast(cb::remove_entities(&[entity_id]));
    let leave_msg = text::system_notice(format!("{name} left the cubeplane"));
    shared.broadcast(cb::system_chat(&leave_msg, false));
    shared.fire_mod(ModEvent::PlayerLeave { player: name.clone() });
    info!(%name, players = shared.player_count(), "player left");

    drop(player); // drop our sender clone so the writer task can finish
    writer_task.abort();
    result
}

async fn play_loop(
    reader: &mut Reader,
    shared: &Arc<Shared>,
    player: &Player,
    threshold: i32,
    loaded: &mut HashSet<(i32, i32)>,
    last_center: &mut (i32, i32),
) -> Result<()> {
    loop {
        let frame = match read_frame(reader, threshold).await {
            Ok(f) => f,
            Err(_) => break,
        };
        if frame.is_empty() {
            continue;
        }
        let raw = RawPacket::parse(frame)?;
        let play = serverbound::parse_play(raw)?;
        match play {
            Play::SetPosition { x, y, z, on_ground } => {
                player.update(|s| {
                    s.x = x;
                    s.y = y;
                    s.z = z;
                    s.on_ground = on_ground;
                });
                broadcast_move(shared, player);
                check_environment(shared, player);
                maybe_stream(shared, player, loaded, last_center);
            }
            Play::SetPositionRotation { x, y, z, yaw, pitch, on_ground } => {
                player.update(|s| {
                    s.x = x;
                    s.y = y;
                    s.z = z;
                    s.yaw = yaw;
                    s.pitch = pitch;
                    s.on_ground = on_ground;
                });
                broadcast_move(shared, player);
                check_environment(shared, player);
                maybe_stream(shared, player, loaded, last_center);
            }
            Play::SetRotation { yaw, pitch, on_ground } => {
                player.update(|s| {
                    s.yaw = yaw;
                    s.pitch = pitch;
                    s.on_ground = on_ground;
                });
                broadcast_move(shared, player);
            }
            Play::OnGround { on_ground } => {
                player.update(|s| s.on_ground = on_ground);
            }
            Play::HeldItem { slot } => {
                if (0..9).contains(&slot) {
                    player.update(|s| s.held_slot = slot as u8);
                }
            }
            Play::ChatMessage { message } => {
                handle_chat(shared, player, &message);
            }
            Play::ChatCommand { command } => {
                handle_command(shared, player, &command);
            }
            Play::BlockDig { status, x, y, z, sequence } => {
                let creative = player.gamemode() == 1;
                if status == 2 || (status == 0 && creative) {
                    break_block(shared, player, x, y, z, creative);
                }
                player.send(cb::acknowledge_block_change(sequence));
            }
            Play::BlockPlace { x, y, z, face, sequence } => {
                place_block(shared, player, x, y, z, face);
                player.send(cb::acknowledge_block_change(sequence));
            }
            Play::UseItem => {
                try_eat(player);
            }
            Play::CreativeSlot { slot, stack } => {
                if slot >= 0 {
                    player.inventory(|inv| inv.set(slot as usize, stack));
                }
            }
            Play::WindowClick { changed, .. } => {
                player.inventory(|inv| {
                    for (slot, stack) in &changed {
                        if *slot >= 0 {
                            inv.set(*slot as usize, *stack);
                        }
                    }
                });
            }
            Play::UseEntity { target, interaction } => {
                if interaction == 1 && !player.is_dead() {
                    let damage = match player.inventory(|inv| inv.held(player.state().held_slot)).def() {
                        Some(d) => match d.kind {
                            item::ItemKind::Weapon(dmg) => dmg,
                            _ => 1.0,
                        },
                        None => 1.0,
                    };
                    mobs::player_attack(shared, player, target, damage);
                }
            }
            Play::ClientCommand { action } => {
                if action == 0 {
                    respawn_player(shared, player, loaded, last_center);
                }
            }
            Play::TeleportConfirm { .. }
            | Play::KeepAlive { .. }
            | Play::ClientSettings { .. }
            | Play::Animation { .. }
            | Play::Ignored => {}
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn list_entry(p: &Player) -> cb::PlayerListEntry {
    cb::PlayerListEntry {
        uuid: p.uuid,
        name: p.name.clone(),
        gamemode: p.gamemode,
        latency: 0,
    }
}

/// Broadcast this player's current position/rotation to everyone else.
fn broadcast_move(shared: &Arc<Shared>, player: &Player) {
    let s = player.state();
    shared.broadcast_except(
        player.entity_id,
        cb::entity_teleport(player.entity_id, s.x, s.y, s.z, s.yaw, s.pitch, s.on_ground),
    );
    shared.broadcast_except(player.entity_id, cb::entity_head_rotation(player.entity_id, s.yaw));
}

/// Re-stream chunks if the player crossed a chunk boundary.
fn maybe_stream(
    shared: &Arc<Shared>,
    player: &Player,
    loaded: &mut HashSet<(i32, i32)>,
    last_center: &mut (i32, i32),
) {
    let (cx, cz) = player.state().chunk();
    if (cx, cz) != *last_center {
        *last_center = (cx, cz);
        player.send(cb::set_center_chunk(cx, cz));
        stream_chunks(shared, player, cx, cz, loaded);
    }
}

/// Load/unload chunks so the player has exactly the view-distance square loaded.
fn stream_chunks(
    shared: &Arc<Shared>,
    player: &Player,
    cx: i32,
    cz: i32,
    loaded: &mut HashSet<(i32, i32)>,
) {
    let r = shared.config.server.view_distance.max(2);
    let mut wanted = HashSet::new();
    for dz in -r..=r {
        for dx in -r..=r {
            wanted.insert((cx + dx, cz + dz));
        }
    }

    // Send newly-needed chunks (closest first for nicer pop-in).
    let mut to_send: Vec<(i32, i32)> = wanted.difference(loaded).copied().collect();
    to_send.sort_by_key(|(x, z)| (x - cx).pow(2) + (z - cz).pow(2));
    for (x, z) in to_send {
        let payload = {
            let mut world = shared.world.lock().unwrap();
            cb::chunk_data(world.chunk(x, z))
        };
        player.send(payload);
        loaded.insert((x, z));
    }

    // Unload chunks that fell outside the view.
    let to_unload: Vec<(i32, i32)> = loaded.difference(&wanted).copied().collect();
    for (x, z) in to_unload {
        player.send(cb::unload_chunk(x, z));
        loaded.remove(&(x, z));
    }
}

/// Apply fall and void damage based on the player's latest position.
fn check_environment(shared: &Arc<Shared>, player: &Player) {
    let s = player.state();
    if s.dead {
        return;
    }

    // Void: below the world kills in any mode.
    if s.y < cubeplane_world::chunk::MIN_Y as f64 - 0.5 {
        combat::damage_player(shared, player, 4.0, "fell out of the world");
        return;
    }

    // Fall damage applies outside creative.
    if player.gamemode() == 1 {
        return;
    }
    if s.on_ground {
        let fall = player.update(|st| {
            let f = st.fall_peak_y - st.y;
            st.fall_peak_y = st.y;
            f
        });
        let damage = (fall - 3.0).floor();
        if damage > 0.0 {
            combat::damage_player(shared, player, damage as f32, "hit the ground too hard");
        }
    } else {
        player.update(|st| {
            if st.y > st.fall_peak_y {
                st.fall_peak_y = st.y;
            }
        });
    }
}

/// Revive the player and rebuild their world view after death.
fn respawn_player(
    shared: &Arc<Shared>,
    player: &Player,
    loaded: &mut HashSet<(i32, i32)>,
    last_center: &mut (i32, i32),
) {
    combat::revive(player);
    let spawn = shared.world.lock().unwrap().spawn();
    let is_flat = shared.world.lock().unwrap().generator_name() == "flat";
    player.update(|s| {
        s.x = spawn.0;
        s.y = spawn.1;
        s.z = spawn.2;
        s.fall_peak_y = spawn.1;
    });

    let gamemode = player.gamemode() as u8;
    player.send(cb::respawn(gamemode, is_flat));
    player.sync_inventory();
    let abilities = if player.gamemode() == 1 { 0x0D } else { 0x00 };
    player.send(cb::player_abilities(abilities, 0.05, 0.1));
    player.send(cb::set_held_item(0));
    player.send(cb::spawn_position(spawn.0 as i32, spawn.1 as i32, spawn.2 as i32, 0.0));
    player.send(cb::sync_position(spawn.0, spawn.1, spawn.2, 0.0, 0.0, 0, 0));

    let (cx, cz) = (
        (spawn.0.floor() as i32).div_euclid(16),
        (spawn.2.floor() as i32).div_euclid(16),
    );
    player.send(cb::set_center_chunk(cx, cz));
    loaded.clear();
    stream_chunks(shared, player, cx, cz, loaded);
    *last_center = (cx, cz);
    player.send(cb::game_event(13, 0.0));

    // Respawn clears entities client-side: re-send other players and mobs.
    for p in shared.players() {
        if p.entity_id == player.entity_id {
            continue;
        }
        let st = p.state();
        player.send(cb::spawn_player(p.entity_id, p.uuid, st.x, st.y, st.z, st.yaw, st.pitch));
        player.send(cb::entity_head_rotation(p.entity_id, st.yaw));
    }
    for m in shared.mobs() {
        player.send(cb::spawn_entity(
            m.entity_id, m.uuid, m.kind.type_id(), m.x, m.y, m.z, m.yaw, m.pitch, m.yaw, 0, (0, 0, 0),
        ));
    }
    broadcast_move(shared, player);
}

fn break_block(shared: &Arc<Shared>, player: &Player, x: i32, y: i32, z: i32, creative: bool) {
    let previous = {
        let mut world = shared.world.lock().unwrap();
        let prev = world.get_block(x, y, z);
        world.set_block(x, y, z, cubeplane_world::block::AIR);
        prev
    };
    shared.broadcast(cb::block_update(x, y, z, cubeplane_world::block::AIR));

    // Survival breaks drop the block's item.
    if !creative {
        if let Some(item_id) = item::item_for_block(previous) {
            drops::spawn_item(shared, item_id, 1, x as f64 + 0.5, y as f64 + 0.25, z as f64 + 0.5, 10);
        }
    }
    shared.fire_mod(ModEvent::BlockBreak {
        player: player.name.clone(),
        x,
        y,
        z,
    });
}

fn place_block(shared: &Arc<Shared>, player: &Player, x: i32, y: i32, z: i32, face: i32) {
    let held = player.state().held_slot;
    let stack = player.inventory(|inv| inv.held(held));
    // Only place if the held item maps to a block.
    let Some(state) = item::block_for_item(stack.id) else {
        return;
    };

    let (dx, dy, dz) = match face {
        0 => (0, -1, 0),
        1 => (0, 1, 0),
        2 => (0, 0, -1),
        3 => (0, 0, 1),
        4 => (-1, 0, 0),
        5 => (1, 0, 0),
        _ => (0, 1, 0),
    };
    let (px, py, pz) = (x + dx, y + dy, z + dz);
    {
        let mut world = shared.world.lock().unwrap();
        world.set_block(px, py, pz, state);
    }
    shared.broadcast(cb::block_update(px, py, pz, state));

    // Survival consumes the placed item.
    if player.gamemode() != 1 {
        let slot = crate::inventory::HOTBAR_START + held as usize;
        let after = player.inventory(|inv| inv.consume_held(held));
        player.send(cb::set_slot(0, 0, slot as i16, after));
    }

    let block_name = cubeplane_world::block::by_name; // for the mod event name
    let name = item::def(stack.id).map(|d| d.name).unwrap_or("block");
    let _ = block_name;
    shared.fire_mod(ModEvent::BlockPlace {
        player: player.name.clone(),
        x: px,
        y: py,
        z: pz,
        block: name.to_string(),
    });
}

/// Eat the held food item if the player is hungry.
fn try_eat(player: &Player) {
    let held = player.state().held_slot;
    let stack = player.inventory(|inv| inv.held(held));
    let Some((hunger, sat)) = stack.def().and_then(|d| match d.kind {
        item::ItemKind::Food(h, s) => Some((h, s)),
        _ => None,
    }) else {
        return;
    };

    let needs_food = player.state().food < 20;
    if !needs_food && player.gamemode() != 1 {
        return;
    }

    // Restore hunger/saturation and consume one item (survival).
    let (health, food, saturation) = player.update(|s| {
        s.food = (s.food + hunger).min(20);
        s.saturation = (s.saturation + sat).min(s.food as f32);
        (s.health, s.food, s.saturation)
    });
    player.send(cb::update_health(health, food, saturation));
    if player.gamemode() != 1 {
        let slot = crate::inventory::HOTBAR_START + held as usize;
        let after = player.inventory(|inv| inv.consume_held(held));
        player.send(cb::set_slot(0, 0, slot as i16, after));
    }
}

fn handle_chat(shared: &Arc<Shared>, player: &Player, message: &str) {
    let line = text::chat_line(&player.name, message);
    shared.broadcast(cb::system_chat(&line, false));
    info!(player = %player.name, "{message}");
    shared.fire_mod(ModEvent::Chat {
        player: player.name.clone(),
        message: message.to_string(),
    });
}

fn handle_command(shared: &Arc<Shared>, player: &Player, command: &str) {
    let mut parts = command.split_whitespace();
    let name = parts.next().unwrap_or("").to_lowercase();
    let args: Vec<String> = parts.map(str::to_string).collect();

    // Built-in commands first; unhandled ones go to the mod runtime.
    if !commands::dispatch(shared, player, &name, &args) {
        shared.fire_mod(ModEvent::Command {
            player: player.name.clone(),
            command: name,
            args,
        });
    }
}
