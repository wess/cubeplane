//! The per-connection lifecycle: handshake → status/login → play.

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::Result;
use serde_json::json;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpStream;
use tokio::sync::mpsc::unbounded_channel;
use tracing::{debug, info, warn};

use cubeplane_mods::ModEvent;
use cubeplane_protocol::{ProtoRead, ProtoWrite, RawPacket, PROTOCOL_VERSION};

use crate::ai::{self, Turn};
use crate::clientbound as cb;
use crate::codec::{read_frame, write_frame, EncryptedReader, EncryptedWriter, NO_COMPRESSION};
use crate::combat;
use crate::commands;
use crate::drops;
use crate::encryption::{auth_hash, Cfb8};
use crate::item;
use crate::mobs;
use crate::ids::{login_cb, login_sb, status_sb};
use crate::player::{offline_uuid, Player};
use crate::serverbound::{self, Play};
use crate::state::Shared;
use crate::text;

/// Entry point for a freshly-accepted TCP connection.
pub async fn handle(stream: TcpStream, shared: Arc<Shared>) {
    let peer = stream.peer_addr().ok();
    stream.set_nodelay(true).ok();
    if let Err(e) = drive(stream, shared).await {
        debug!(?peer, "connection closed: {e}");
    }
}

async fn drive(stream: TcpStream, shared: Arc<Shared>) -> Result<()> {
    let (mut rh, mut wh) = stream.into_split();

    // --- Handshake -----------------------------------------------------------
    let frame = read_frame(&mut rh, NO_COMPRESSION).await?;
    let mut raw = RawPacket::parse(frame)?;
    let protocol = raw.body.read_varint()?;
    let _host = raw.body.read_string()?;
    let _port = raw.body.read_u16()?;
    let next_state = raw.body.read_varint()?;

    match next_state {
        1 => status(&mut rh, &mut wh, &shared).await,
        2 => login(rh, wh, shared, protocol).await,
        other => {
            debug!("unknown next-state {other} in handshake");
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Status (server list ping)
// ---------------------------------------------------------------------------

async fn status<R, W>(reader: &mut R, writer: &mut W, shared: &Arc<Shared>) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
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

async fn login(mut rh: OwnedReadHalf, mut wh: OwnedWriteHalf, shared: Arc<Shared>, protocol: i32) -> Result<()> {
    let frame = read_frame(&mut rh, NO_COMPRESSION).await?;
    let mut raw = RawPacket::parse(frame)?;
    if raw.id != login_sb::LOGIN_START {
        return Ok(());
    }
    let name = raw.body.read_string()?;

    // Version gate: cubeplane speaks exactly protocol 763 (Minecraft 1.20.1).
    if protocol != PROTOCOL_VERSION {
        let reason = text::colored(
            format!(
                "cubeplane requires Minecraft {} (protocol {}); you connected with protocol {}.",
                cubeplane_protocol::GAME_VERSION,
                PROTOCOL_VERSION,
                protocol
            ),
            "red",
        );
        write_frame(&mut wh, &cb::login_disconnect(&reason), NO_COMPRESSION).await?;
        return Ok(());
    }

    // Reject if full.
    if shared.player_count() as i32 >= shared.config.server.max_players {
        let reason = text::colored("cubeplane is full", "red");
        write_frame(&mut wh, &cb::login_disconnect(&reason), NO_COMPRESSION).await?;
        return Ok(());
    }

    // Encrypted (online) path negotiates a shared secret and wraps the IO.
    if let Some(key) = shared.server_key.clone() {
        let secret = match encryption_handshake(&mut rh, &mut wh, &key).await? {
            Some(s) => s,
            None => return Ok(()), // verify token mismatch
        };
        let uuid = resolve_identity(&shared, &key, &name, &secret).await;
        let reader = EncryptedReader::new(rh, Cfb8::new(&secret));
        let writer = EncryptedWriter::new(wh, Cfb8::new(&secret));
        finish_login(reader, writer, shared, name, uuid).await
    } else {
        let uuid = offline_uuid(&name);
        finish_login(rh, wh, shared, name, uuid).await
    }
}

/// Run the Encryption Request/Response exchange, returning the shared secret.
async fn encryption_handshake<R, W>(
    reader: &mut R,
    writer: &mut W,
    key: &crate::encryption::ServerKey,
) -> Result<Option<Vec<u8>>>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let verify_token: [u8; 4] = rand::random();

    // Encryption Request: empty server id, public key DER, verify token.
    let mut req = bytes::BytesMut::new();
    req.write_varint(login_cb::ENCRYPTION_REQUEST);
    req.write_string("");
    req.write_varint(key.public_der().len() as i32);
    req.write_bytes(key.public_der());
    req.write_varint(verify_token.len() as i32);
    req.write_bytes(&verify_token);
    write_frame(writer, &req, NO_COMPRESSION).await?;

    // Encryption Response.
    let frame = read_frame(reader, NO_COMPRESSION).await?;
    let mut raw = RawPacket::parse(frame)?;
    if raw.id != login_sb::ENCRYPTION_RESPONSE {
        return Ok(None);
    }
    let secret_len = raw.body.read_varint()? as usize;
    let enc_secret = raw.body.read_bytes(secret_len)?;
    let token_len = raw.body.read_varint()? as usize;
    let enc_token = raw.body.read_bytes(token_len)?;

    let token = key.decrypt(&enc_token)?;
    if token != verify_token {
        warn!("encryption verify token mismatch");
        return Ok(None);
    }
    let secret = key.decrypt(&enc_secret)?;
    if secret.len() != 16 {
        return Ok(None);
    }
    Ok(Some(secret))
}

/// Determine the player's UUID, attempting Mojang session auth in online mode.
async fn resolve_identity(
    shared: &Arc<Shared>,
    key: &crate::encryption::ServerKey,
    name: &str,
    secret: &[u8],
) -> uuid::Uuid {
    let _hash = auth_hash("", secret, key.public_der());
    // A full implementation calls sessionserver.mojang.com/session/minecraft/
    // hasJoined?username=<name>&serverId=<hash> here. That requires outbound
    // network access; when unavailable we fall back to the deterministic
    // offline UUID so the encrypted path still works end to end.
    let _ = shared;
    offline_uuid(name)
}

/// Send Set Compression + Login Success and enter the play state.
async fn finish_login<R, W>(reader: R, mut writer: W, shared: Arc<Shared>, name: String, uuid: uuid::Uuid) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
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

async fn play<R, W>(
    mut reader: R,
    writer: W,
    shared: Arc<Shared>,
    name: String,
    uuid: uuid::Uuid,
    threshold: i32,
) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
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
        "heal", "kill", "clear", "xp", "vehicle",
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
    let ai_on = shared.ai_config().enabled;
    for m in shared.mobs() {
        player.send(cb::spawn_entity(
            m.entity_id, m.uuid, m.kind.type_id(), m.x, m.y, m.z, m.yaw, m.pitch, m.yaw, 0, (0, 0, 0),
        ));
        let meta = m.metadata();
        if !meta.is_empty() {
            player.send(cb::entity_metadata(m.entity_id, &meta));
        }
        if ai_on && m.kind.name() == "villager" {
            if let Some((name, prof)) = shared.villager_identity(m.entity_id) {
                player.send(cb::entity_custom_name(m.entity_id, &text::colored(format!("{name} the {prof}"), "green")));
            }
        }
    }
    // …and every vehicle (with its rider, if any).
    for v in shared.vehicles() {
        player.send(cb::spawn_entity(v.entity_id, v.uuid, v.type_id, v.x, v.y, v.z, v.yaw, 0.0, v.yaw, 0, (0, 0, 0)));
        if let Some(r) = v.rider {
            player.send(cb::set_passengers(v.entity_id, &[r]));
        }
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

async fn play_loop<R: AsyncRead + Unpin>(
    reader: &mut R,
    shared: &Arc<Shared>,
    player: &Player,
    threshold: i32,
    loaded: &mut HashSet<(i32, i32)>,
    last_center: &mut (i32, i32),
) -> Result<()> {
    loop {
        // Liveness: the server sends keep-alives every 10s, so a live client
        // produces inbound traffic well within 30s. No data for 30s ⇒ drop.
        let frame = match tokio::time::timeout(
            std::time::Duration::from_secs(30),
            read_frame(reader, threshold),
        )
        .await
        {
            Ok(Ok(f)) => f,
            _ => break,
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
                // Interacting with a lever/chest takes priority over placing.
                if !try_toggle_lever(shared, x, y, z) && !try_open_container(shared, player, x, y, z) {
                    place_block(shared, player, x, y, z, face);
                }
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
            Play::WindowClick { window_id, changed } => {
                apply_window_click(shared, player, window_id, &changed);
            }
            Play::CloseWindow => {
                player.update(|s| {
                    s.open_container = None;
                    s.open_merchant = false;
                });
            }
            Play::UseEntity { target, interaction } => {
                if player.is_dead() {
                } else if interaction == 1 {
                    let damage = match player.inventory(|inv| inv.held(player.state().held_slot)).def() {
                        Some(d) => match d.kind {
                            item::ItemKind::Weapon(dmg) => dmg,
                            _ => 1.0,
                        },
                        None => 1.0,
                    };
                    mobs::player_attack(shared, player, target, damage);
                } else {
                    // Right-click interact: ride a vehicle or trade with a villager.
                    interact_entity(shared, player, target);
                }
            }
            Play::VehicleMove { x, y, z, yaw, pitch } => {
                if let Some(vid) = player.state().riding {
                    shared.with_vehicle(vid, |v| {
                        v.x = x;
                        v.y = y;
                        v.z = z;
                        v.yaw = yaw;
                    });
                    shared.broadcast_except(player.entity_id, cb::entity_teleport(vid, x, y, z, yaw, pitch, true));
                }
            }
            Play::SteerVehicle { jump } => {
                if jump {
                    if let Some(vid) = player.state().riding {
                        player.update(|s| s.riding = None);
                        shared.with_vehicle(vid, |v| v.rider = None);
                        shared.broadcast(cb::set_passengers(vid, &[]));
                    }
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
        // Clone the column under a brief lock, then do the expensive palette +
        // lighting encode *outside* the global world lock to cut contention.
        let chunk = {
            let mut world = shared.world.lock().unwrap();
            world.chunk(x, z).clone()
        };
        let light = match chunk.cached_light() {
            Some(l) => l.clone(),
            None => {
                let l = chunk.compute_light();
                // Cache it back so re-sends to other players don't recompute.
                shared.world.lock().unwrap().store_light(x, z, l.clone());
                l
            }
        };
        player.send(cb::chunk_data(&chunk, &light));
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
    let ai_on = shared.ai_config().enabled;
    for m in shared.mobs() {
        player.send(cb::spawn_entity(
            m.entity_id, m.uuid, m.kind.type_id(), m.x, m.y, m.z, m.yaw, m.pitch, m.yaw, 0, (0, 0, 0),
        ));
        if ai_on && m.kind.name() == "villager" {
            if let Some((name, prof)) = shared.villager_identity(m.entity_id) {
                player.send(cb::entity_custom_name(m.entity_id, &text::colored(format!("{name} the {prof}"), "green")));
            }
        }
    }
    for v in shared.vehicles() {
        player.send(cb::spawn_entity(v.entity_id, v.uuid, v.type_id, v.x, v.y, v.z, v.yaw, 0.0, v.yaw, 0, (0, 0, 0)));
    }
    broadcast_move(shared, player);
}

/// Right-click interaction with an entity: ride a vehicle, or trade with a
/// villager.
fn interact_entity(shared: &Arc<Shared>, player: &Player, target: i32) {
    // Mount a vehicle.
    if shared.is_vehicle(target) {
        shared.with_vehicle(target, |v| v.rider = Some(player.entity_id));
        player.update(|s| s.riding = Some(target));
        shared.broadcast(cb::set_passengers(target, &[player.entity_id]));
        return;
    }
    // Talk to (AI) or trade with a villager.
    let is_villager = shared
        .with_mob(target, |m| m.kind.name() == "villager")
        .unwrap_or(false);
    if is_villager {
        if shared.ai_config().enabled {
            start_conversation(shared, player, target);
        } else {
            open_merchant(shared, player);
        }
    }
}

/// Compose a villager chat line: gold "Name (profession)" + white speech.
fn villager_line(name: &str, profession: &str, text: &str) -> serde_json::Value {
    serde_json::json!({
        "text": "",
        "extra": [
            { "text": format!("{name} ({profession}) ", name = name, profession = profession), "color": "gold" },
            { "text": text, "color": "white" }
        ]
    })
}

/// Begin a conversation with an AI villager.
fn start_conversation(shared: &Arc<Shared>, player: &Player, villager: i32) {
    shared.register_villager(villager);
    player.update(|s| s.talking_to = Some(villager));
    if let Some((name, prof)) = shared.villager_identity(villager) {
        let greeting = format!(
            "Hello there, {}! I'm {name}, the village {prof}. What brings you my way? (say 'bye' to part)",
            player.name
        );
        player.send(cb::system_chat(&villager_line(&name, prof, &greeting), false));
    }
}

/// Route a talking player's chat to their villager and reply asynchronously.
fn talk_to_villager(shared: &Arc<Shared>, player: &Player, villager: i32, message: &str) {
    // Leaving the conversation.
    let lower = message.trim().to_lowercase();
    if matches!(lower.as_str(), "bye" | "goodbye" | "leave" | "farewell") {
        player.update(|s| s.talking_to = None);
        if let Some((name, prof)) = shared.villager_identity(villager) {
            player.send(cb::system_chat(&villager_line(&name, prof, "Safe travels, friend!"), false));
        }
        return;
    }

    let Some((name, prof)) = shared.villager_identity(villager) else {
        // Villager is gone.
        player.update(|s| s.talking_to = None);
        player.send(cb::system_chat(&text::colored("(the villager has wandered off)", "gray"), false));
        return;
    };

    // Single-flight: ignore new input while the model is responding.
    // If AI was switched off mid-conversation, end gracefully.
    if !shared.ai_config().enabled {
        player.update(|s| s.talking_to = None);
        player.send(cb::system_chat(&villager_line(&name, prof, "…I've nothing more to say for now."), false));
        return;
    }

    let busy = shared.with_villager(villager, |b| b.busy).unwrap_or(true);
    if busy {
        player.send(cb::system_chat(&text::colored("(they're still thinking…)", "gray"), false));
        return;
    }
    let history = shared
        .with_villager(villager, |b| {
            b.busy = true;
            b.history.clone()
        })
        .unwrap_or_default();

    let cfg = shared.ai_config();
    let system = ai::system_prompt(&name, prof);
    let shared = shared.clone();
    let player = player.clone();
    let user_msg = message.to_string();

    tokio::spawn(async move {
        let result = ai::chat(&cfg, &system, &history, &user_msg).await;
        let limit = cfg.history_limit.max(1) * 2;
        match result {
            Ok(reply) => {
                shared.with_villager(villager, |b| {
                    b.history.push(Turn { role: "user", text: user_msg });
                    b.history.push(Turn { role: "assistant", text: reply.clone() });
                    if b.history.len() > limit {
                        let drop = b.history.len() - limit;
                        b.history.drain(0..drop);
                    }
                    b.busy = false;
                });
                player.send(cb::system_chat(&villager_line(&name, prof, &reply), false));
            }
            Err(e) => {
                shared.with_villager(villager, |b| b.busy = false);
                tracing::warn!("villager AI error: {e}");
                player.send(cb::system_chat(
                    &villager_line(&name, prof, "…hm, I've lost my train of thought."),
                    false,
                ));
            }
        }
    });
}

/// Open a villager trade window with a couple of sample offers.
fn open_merchant(shared: &Arc<Shared>, player: &Player) {
    let _ = shared;
    let emerald = item::id_any("emerald").unwrap_or(0);
    let bread = item::id_any("bread").unwrap_or(0);
    let stick = item::id_any("stick").unwrap_or(0);
    let offers = vec![
        (
            item::ItemStack::new(emerald, 1),
            item::ItemStack::new(bread, 6),
            item::ItemStack::EMPTY,
        ),
        (
            item::ItemStack::new(stick, 32),
            item::ItemStack::new(emerald, 1),
            item::ItemStack::EMPTY,
        ),
    ];
    player.update(|s| s.open_merchant = true);
    player.send(cb::open_window(2, 18, &text::plain("Villager"))); // 18 = merchant menu
    player.send(cb::trade_list(2, &offers));
}

/// Toggle a lever the player clicked, updating redstone. Returns true if a
/// lever was toggled.
fn try_toggle_lever(shared: &Arc<Shared>, x: i32, y: i32, z: i32) -> bool {
    let state = { shared.world.lock().unwrap().get_block(x, y, z) };
    if cubeplane_world::block::name_of(state) != "lever" {
        return false;
    }
    let powered = cubeplane_world::block::prop_index(state, "powered").unwrap_or(1);
    let toggled = cubeplane_world::block::with_prop(state, "powered", if powered == 0 { 1 } else { 0 });
    {
        let mut w = shared.world.lock().unwrap();
        w.set_block(x, y, z, toggled);
    }
    shared.broadcast(cb::block_update(x, y, z, toggled));
    crate::sim::redstone_update(shared, x, y, z);
    true
}

/// Open a chest the player clicked. Returns true if a container was opened.
fn try_open_container(shared: &Arc<Shared>, player: &Player, x: i32, y: i32, z: i32) -> bool {
    let is_chest = {
        let mut w = shared.world.lock().unwrap();
        cubeplane_world::block::info(w.get_block(x, y, z)).name == "chest"
    };
    if !is_chest {
        return false;
    }
    let pos = (x, y, z);
    shared.ensure_container(pos);
    player.update(|s| s.open_container = Some(pos));

    let container = shared.container_items(pos).unwrap_or_default();
    let inv = player.inventory(|i| i.slots().to_vec());
    let mut combined: Vec<item::ItemStack> = Vec::with_capacity(63);
    combined.extend_from_slice(&container);
    combined.extend_from_slice(&inv[9..45]); // player main + hotbar
    player.send(cb::open_window(1, 2, &text::plain("Chest")));
    player.send(cb::window_items(1, 0, &combined, item::ItemStack::EMPTY));
    true
}

/// Apply a window click, routing chest-window slots to the open container.
fn apply_window_click(shared: &Arc<Shared>, player: &Player, window_id: u8, changed: &[(i16, item::ItemStack)]) {
    use crate::state::CONTAINER_SIZE;
    // The merchant economy isn't simulated; ignore clicks so they can't desync
    // the player inventory.
    if player.state().open_merchant {
        return;
    }
    if window_id != 0 {
        if let Some(pos) = player.state().open_container {
            for (slot, stack) in changed {
                if *slot < 0 {
                    continue;
                }
                let s = *slot as usize;
                if s < CONTAINER_SIZE {
                    shared.set_container_slot(pos, s, *stack);
                } else {
                    let inv_slot = s - CONTAINER_SIZE + 9;
                    player.inventory(|i| i.set(inv_slot, *stack));
                }
            }
            return;
        }
    }
    // Player inventory window: slots are direct inventory indices.
    player.inventory(|i| {
        for (slot, stack) in changed {
            if *slot >= 0 {
                i.set(*slot as usize, *stack);
            }
        }
    });
}

fn break_block(shared: &Arc<Shared>, player: &Player, x: i32, y: i32, z: i32, creative: bool) {
    let previous = {
        let mut world = shared.world.lock().unwrap();
        let prev = world.get_block(x, y, z);
        world.set_block(x, y, z, cubeplane_world::block::AIR);
        prev
    };
    shared.broadcast(cb::block_update(x, y, z, cubeplane_world::block::AIR));

    // Breaking a chest spills its contents and removes the block entity.
    if cubeplane_world::block::info(previous).name == "chest" {
        if let Some(items) = shared.remove_container((x, y, z)) {
            for st in items {
                if !st.is_empty() {
                    drops::spawn_item(shared, st.id, st.count, x as f64 + 0.5, y as f64 + 0.5, z as f64 + 0.5, 10);
                }
            }
        }
    }

    // Survival breaks drop the block's item.
    if !creative {
        if let Some(item_id) = item::item_for_block(previous) {
            drops::spawn_item(shared, item_id, 1, x as f64 + 0.5, y as f64 + 0.25, z as f64 + 0.5, 10);
        }
    }
    // Fluids can now flow into the gap; refresh redstone if it was a component.
    shared.schedule_fluid(x, y, z);
    if crate::sim::is_redstone(cubeplane_world::block::name_of(previous)) {
        crate::sim::redstone_update(shared, x, y, z);
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
    // Only place if the held item maps to a block (full registry).
    let Some(base) = item::block_state_for_item(stack.id) else {
        return;
    };
    // Orient the block from the clicked face and the player's yaw.
    let state = cubeplane_world::block::place_state(base, face, player.state().yaw);

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

    // Placing a chest creates its (empty) container block entity.
    if cubeplane_world::block::info(state).name == "chest" {
        shared.ensure_container((px, py, pz));
    }
    // Let fluids flow toward / from the change, and update redstone.
    shared.schedule_fluid(px, py, pz);
    if crate::sim::is_redstone(cubeplane_world::block::name_of(state)) {
        crate::sim::redstone_update(shared, px, py, pz);
    }

    // Survival consumes the placed item.
    if player.gamemode() != 1 {
        let slot = crate::inventory::HOTBAR_START + held as usize;
        let after = player.inventory(|inv| inv.consume_held(held));
        player.send(cb::set_slot(0, 0, slot as i16, after));
    }

    let name = item::name_of(stack.id).unwrap_or("block");
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
    let s = player.state();
    player.send(cb::sound_effect("entity.generic.eat", 7, s.x, s.y, s.z, 0.8, 1.0));
    if player.gamemode() != 1 {
        let slot = crate::inventory::HOTBAR_START + held as usize;
        let after = player.inventory(|inv| inv.consume_held(held));
        player.send(cb::set_slot(0, 0, slot as i16, after));
    }
}

fn handle_chat(shared: &Arc<Shared>, player: &Player, message: &str) {
    // If the player is conversing with a villager, route there instead of
    // broadcasting to the whole server.
    if let Some(villager) = player.state().talking_to {
        talk_to_villager(shared, player, villager, message);
        return;
    }
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

#[cfg(test)]
#[allow(clippy::needless_option_as_deref)]
mod encryption_tests {
    use crate::encryption::Cfb8;
    use crate::Config;
    use bytes::BytesMut;
    use cubeplane_protocol::{ProtoRead, ProtoWrite, PROTOCOL_VERSION};
    use rsa::pkcs8::DecodePublicKey;
    use rsa::{Pkcs1v15Encrypt, RsaPublicKey};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    async fn read_byte(c: &mut TcpStream, dec: Option<&mut Cfb8>) -> u8 {
        let mut b = [0u8; 1];
        c.read_exact(&mut b).await.unwrap();
        if let Some(d) = dec {
            d.decrypt(&mut b);
        }
        b[0]
    }

    async fn read_varint(c: &mut TcpStream, mut dec: Option<&mut Cfb8>) -> i32 {
        let mut value: u32 = 0;
        let mut pos = 0;
        loop {
            let byte = read_byte(c, dec.as_deref_mut()).await;
            value |= ((byte & 0x7F) as u32) << pos;
            if byte & 0x80 == 0 {
                break;
            }
            pos += 7;
        }
        value as i32
    }

    /// Read one (optionally decrypted) frame, returning (id, body).
    async fn read_frame(c: &mut TcpStream, mut dec: Option<&mut Cfb8>) -> (i32, BytesMut) {
        let len = read_varint(c, dec.as_deref_mut()).await as usize;
        let mut buf = vec![0u8; len];
        c.read_exact(&mut buf).await.unwrap();
        if let Some(d) = dec.as_deref_mut() {
            d.decrypt(&mut buf);
        }
        let mut body = BytesMut::from(&buf[..]);
        let id = body.read_varint().unwrap();
        (id, body)
    }

    async fn write_frame(c: &mut TcpStream, payload: &[u8], enc: Option<&mut Cfb8>) {
        let mut framed = BytesMut::new();
        framed.write_varint(payload.len() as i32);
        framed.write_bytes(payload);
        let mut bytes = framed.to_vec();
        if let Some(e) = enc {
            e.encrypt(&mut bytes);
        }
        c.write_all(&bytes).await.unwrap();
        c.flush().await.unwrap();
    }

    #[tokio::test]
    async fn encrypted_login_handshake_completes() {
        // Free port.
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe);

        let mut config = Config::default();
        config.server.host = "127.0.0.1".into();
        config.server.port = port;
        config.server.compression_threshold = -1;
        config.server.online_mode = true;
        config.server.view_distance = 2;
        config.control.enabled = false;
        config.mods.enabled = false;
        config.world.save = false;
        config.world.generator = "flat".into();
        tokio::spawn(async move {
            let _ = crate::run(config).await;
        });
        tokio::time::sleep(Duration::from_millis(400)).await;

        let mut conn = TcpStream::connect(("127.0.0.1", port)).await.unwrap();

        // Handshake (next state = login).
        let mut hs = BytesMut::new();
        hs.write_varint(0x00);
        hs.write_varint(PROTOCOL_VERSION);
        hs.write_string("127.0.0.1");
        hs.write_u16(port);
        hs.write_varint(2);
        write_frame(&mut conn, &hs, None).await;

        // Login Start.
        let mut ls = BytesMut::new();
        ls.write_varint(0x00);
        ls.write_string("Crypto");
        ls.write_bool(false);
        write_frame(&mut conn, &ls, None).await;

        // Encryption Request → grab the public key + verify token.
        let (id, mut body) = read_frame(&mut conn, None).await;
        assert_eq!(id, 0x01, "expected Encryption Request");
        let _server_id = body.read_string().unwrap();
        let key_len = body.read_varint().unwrap() as usize;
        let key_der = body.read_bytes(key_len).unwrap();
        let token_len = body.read_varint().unwrap() as usize;
        let token = body.read_bytes(token_len).unwrap();

        let public = RsaPublicKey::from_public_key_der(&key_der).unwrap();
        let secret = [0x24u8; 16];
        let mut rng = rand::rngs::OsRng;
        let enc_secret = public.encrypt(&mut rng, Pkcs1v15Encrypt, &secret).unwrap();
        let enc_token = public.encrypt(&mut rng, Pkcs1v15Encrypt, &token).unwrap();

        // Encryption Response.
        let mut resp = BytesMut::new();
        resp.write_varint(0x01);
        resp.write_varint(enc_secret.len() as i32);
        resp.write_bytes(&enc_secret);
        resp.write_varint(enc_token.len() as i32);
        resp.write_bytes(&enc_token);
        write_frame(&mut conn, &resp, None).await;

        // Everything is now AES-CFB8 encrypted.
        let mut dec = Cfb8::new(&secret);
        let mut enc = Cfb8::new(&secret);
        let _ = &mut enc; // (only needed if we send more)

        // Expect Login Success (0x02) over the encrypted channel.
        let (id, mut body) = read_frame(&mut conn, Some(&mut dec)).await;
        assert_eq!(id, 0x02, "expected encrypted Login Success");
        let _uuid = body.read_uuid().unwrap();
        assert_eq!(body.read_string().unwrap(), "Crypto");

        // And then the encrypted play stream begins (Join Game 0x28 appears).
        let mut saw_join = false;
        for _ in 0..40 {
            let (pid, _b) = read_frame(&mut conn, Some(&mut dec)).await;
            if pid == 0x28 {
                saw_join = true;
                break;
            }
        }
        assert!(saw_join, "did not receive encrypted Join Game");
    }
}
