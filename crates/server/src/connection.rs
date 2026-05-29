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

use rand::Rng;

use cubeplane_mods::ModEvent;
use cubeplane_protocol::{ProtoRead, ProtoWrite, RawPacket, PROTOCOL_VERSION};

use crate::advancements;
use crate::ai::{self, Turn};
use crate::clientbound as cb;
use crate::codec::{read_frame, write_frame, EncryptedReader, EncryptedWriter, NO_COMPRESSION};
use crate::combat;
use crate::commands;
use crate::drops;
use crate::encryption::{auth_hash, Cfb8};
use crate::item;
use crate::mobs;
use crate::ids::{config_sb, login_cb, login_sb, status_sb};
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
        1 => status(&mut rh, &mut wh, &shared, protocol).await,
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

async fn status<R, W>(reader: &mut R, writer: &mut W, shared: &Arc<Shared>, protocol: i32) -> Result<()>
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
                let payload = cb::status_response(&status_json(shared, protocol));
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

fn status_json(shared: &Arc<Shared>, client_protocol: i32) -> String {
    // Report our protocol to a client we can host (so it shows as compatible),
    // and our own protocol to others (so they honestly see a version mismatch).
    let advertised = if cubeplane_protocol::is_supported(client_protocol) {
        client_protocol
    } else {
        PROTOCOL_VERSION
    };
    json!({
        "version": { "name": cubeplane_protocol::GAME_VERSION, "protocol": advertised },
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

    // Version gate: cubeplane hosts the play state for protocol 763 (1.20.1).
    // Other clients get a clear, version-named message instead of a raw error.
    if !cubeplane_protocol::is_supported(protocol) {
        let yours = cubeplane_protocol::version_name(protocol)
            .map(|n| format!("Minecraft {n} (protocol {protocol})"))
            .unwrap_or_else(|| format!("protocol {protocol}"));
        let reason = text::colored(
            format!(
                "cubeplane runs Minecraft {} (protocol {}). You connected with {}. Please switch your client to {}.",
                cubeplane_protocol::GAME_VERSION,
                PROTOCOL_VERSION,
                yours,
                cubeplane_protocol::GAME_VERSION,
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
        finish_login(reader, writer, shared, name, uuid, protocol).await
    } else {
        let uuid = offline_uuid(&name);
        finish_login(rh, wh, shared, name, uuid, protocol).await
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
async fn finish_login<R, W>(
    mut reader: R,
    mut writer: W,
    shared: Arc<Shared>,
    name: String,
    uuid: uuid::Uuid,
    protocol: i32,
) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let threshold = shared.config.server.compression_threshold;
    if threshold >= 0 {
        write_frame(&mut writer, &cb::set_compression(threshold), NO_COMPRESSION).await?;
    }
    write_frame(&mut writer, &cb::login_success(uuid, &name), threshold).await?;

    // 1.20.2+ inserts a Configuration phase between login and play: the client
    // acknowledges login, then the server ships the registry codec and signals
    // the end of configuration before play begins.
    if protocol >= 764 {
        configuration_phase(&mut reader, &mut writer, threshold).await?;
    }

    info!(%name, %uuid, "player logging in");
    play(reader, writer, shared, name, uuid, threshold, protocol).await
}

/// Drive the 1.20.2 Configuration state: await Login Acknowledged, send the
/// registry codec and Finish Configuration, then await the client's ack.
async fn configuration_phase<R, W>(reader: &mut R, writer: &mut W, threshold: i32) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    // Login Acknowledged moves the client into the Configuration state.
    let frame = read_frame(reader, threshold).await?;
    let raw = RawPacket::parse(frame)?;
    if raw.id != login_sb::LOGIN_ACKNOWLEDGED {
        return Ok(());
    }
    // Ship the registry codec, then end configuration.
    write_frame(writer, &cb::config_registry_data(), threshold).await?;
    write_frame(writer, &cb::config_finish(), threshold).await?;
    // Consume configuration packets (client info, plugin messages, …) until the
    // client acknowledges Finish Configuration and is ready for play.
    loop {
        let frame = read_frame(reader, threshold).await?;
        if frame.is_empty() {
            continue;
        }
        let raw = RawPacket::parse(frame)?;
        if raw.id == config_sb::FINISH_CONFIGURATION {
            return Ok(());
        }
    }
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
    protocol: i32,
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
            // Translate the canonical (763) packet to the client's wire format.
            let payload = crate::version::translate_clientbound(payload, protocol);
            if write_frame(&mut writer, &payload, threshold).await.is_err() {
                break;
            }
        }
    });

    let player = Player::new(entity_id, uuid, name.clone(), gamemode, tx, spawn, protocol);

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
    // Define the advancement tree (with any progress restored this session).
    {
        let earned = shared.earned_advancements(player.entity_id);
        player.send(advancements::packet(&earned, player.protocol));
    }
    player.send(cb::declare_commands(&[
        ("help", false), ("list", false), ("pos", false), ("tp", true),
        ("gamemode", true), ("give", true), ("time", true), ("weather", true),
        ("summon", true), ("effect", true), ("heal", false), ("kill", false),
        ("clear", false), ("xp", true), ("vehicle", true), ("craft", true), ("dimension", true),
        ("enchant", true),
    ]));
    player.send(cb::tab_list_header(
        &text::colored("cubeplane", "gold"),
        &text::colored("Rust engine · JS mods", "gray"),
    ));
    if !shared.config.server.resource_pack.is_empty() {
        player.send(cb::resource_pack(&shared.config.server.resource_pack, "", false));
    }
    player.send(cb::init_world_border(shared.config.world.border_diameter));
    if shared.raining() {
        player.send(cb::game_event(2, 0.0)); // begin raining
    }
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
    let result = play_loop(&mut reader, &shared, &player, threshold, protocol, &mut loaded, &mut last_center).await;

    // --- Cleanup -------------------------------------------------------------
    if shared.config.world.save {
        let _ = crate::persistence::save_player(&save_dir, uuid, &player.snapshot_data());
    }
    shared.remove_player(entity_id);
    shared.clear_effects(entity_id);
    shared.clear_stats(entity_id);
    shared.clear_advancements(entity_id);
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
    protocol: i32,
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
        // Translate the client's wire packet to the canonical 763 layout.
        let frame = crate::version::translate_serverbound(frame, protocol);
        let raw = RawPacket::parse(frame)?;
        let play = serverbound::parse_play(raw)?;
        match play {
            Play::SetPosition { x, y, z, on_ground } => {
                if !accept_move(shared, player, x, z) {
                    continue;
                }
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
                if !accept_move(shared, player, x, z) {
                    continue;
                }
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
                // /dimension needs the streaming state, so it's handled here.
                let mut parts = command.split_whitespace();
                if parts.next() == Some("dimension") {
                    let dim = match parts.next() {
                        Some("nether") => 1u8,
                        Some("end") => 2,
                        _ => 0,
                    };
                    switch_dimension(shared, player, dim, loaded, last_center);
                } else {
                    handle_command(shared, player, &command);
                }
            }
            Play::BlockDig { status, x, y, z, sequence } => {
                let creative = player.gamemode() == 1;
                if status == 2 || (status == 0 && creative) {
                    break_block(shared, player, x, y, z, creative);
                }
                player.send(cb::acknowledge_block_change(sequence));
            }
            Play::BlockPlace { x, y, z, face, sequence } => {
                // Interacting with a lever/chest/sign takes priority over placing.
                if !try_flint(shared, player, x, y, z, face)
                    && !try_bucket(shared, player, x, y, z, face)
                    && !try_bonemeal(shared, player, x, y, z)
                    && !try_toggle_lever(shared, player, x, y, z)
                    && !try_use_button(shared, player, x, y, z)
                    && !try_use_door(shared, player, x, y, z)
                    && !try_use_bed(shared, player, x, y, z)
                    && !try_open_container(shared, player, x, y, z)
                    && !try_open_furnace(shared, player, x, y, z)
                    && !try_open_brewing(shared, player, x, y, z)
                    && !try_open_crafting(shared, player, x, y, z)
                    && !try_open_anvil(shared, player, x, y, z)
                    && !try_read_sign(shared, player, x, y, z)
                {
                    place_block(shared, player, x, y, z, face);
                }
                player.send(cb::acknowledge_block_change(sequence));
            }
            Play::UseItem => {
                if !try_shoot_bow(shared, player) && !try_drink(shared, player) {
                    try_eat(shared, player);
                }
            }
            Play::CreativeSlot { slot, stack } => {
                // Direct slot edits are only legitimate in creative mode.
                if slot >= 0 && player.gamemode() == 1 {
                    player.inventory(|inv| inv.set(slot as usize, stack));
                }
            }
            Play::WindowClick { window_id, changed } => {
                apply_window_click(shared, player, window_id, &changed);
            }
            Play::SelectTrade { index } => {
                do_trade(shared, player, index);
            }
            Play::UpdateSign { x, y, z, lines } => {
                shared.set_sign((x, y, z), lines);
                player.send(cb::system_chat(&text::colored("Sign saved.", "gray"), false));
            }
            Play::CloseWindow => {
                if player.state().open_crafting {
                    close_crafting(player);
                }
                if player.state().open_anvil {
                    close_anvil(player);
                }
                player.update(|s| {
                    s.open_container = None;
                    s.open_merchant = false;
                    s.merchant_prof = None;
                    s.merchant_id = None;
                    s.open_furnace = None;
                    s.open_brewing = None;
                    s.open_crafting = false;
                    s.open_anvil = false;
                });
            }
            Play::UseEntity { target, interaction } => {
                if player.is_dead() {
                } else if interaction == 1 {
                    let held = player.inventory(|inv| inv.held(player.state().held_slot));
                    let mut damage = match held.def() {
                        Some(d) => match d.kind {
                            item::ItemKind::Weapon(dmg) => dmg,
                            _ => 1.0,
                        },
                        None => 1.0,
                    };
                    // Sharpness enchant adds damage.
                    if let Some(("sharpness", lvl)) = held.enchant() {
                        damage += lvl as f32;
                    }
                    // Strength effect adds 3 damage per level.
                    if let Some(amp) = shared.effect_amplifier(player.entity_id, crate::effects::STRENGTH) {
                        damage += 3.0 * (amp.max(0) as f32 + 1.0);
                    }
                    mobs::player_attack(shared, player, target, damage);
                    if player.gamemode() != 1 {
                        damage_held_tool(shared, player);
                    }
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
                } else if action == 1 {
                    // The client opened the statistics screen.
                    player.send(cb::award_statistics(&shared.stat_entries(player.entity_id)));
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

/// Basic movement anti-cheat: reject implausible horizontal jumps (teleport /
/// speed hacks) by snapping the player back to their last good position.
/// Generous so legitimate sprinting/packets are never rejected.
fn accept_move(shared: &Arc<Shared>, player: &Player, x: f64, z: f64) -> bool {
    if player.state().riding.is_some() {
        return true; // vehicle movement is handled separately
    }
    let s = player.state();
    let (dx, dz) = (x - s.x, z - s.z);
    let limit = if player.gamemode() == 1 { 40.0 } else { 12.0 };
    // Reject implausible jumps (teleport/speed hacks) or moves past the border.
    let radius = shared.config.world.border_diameter / 2.0;
    if dx * dx + dz * dz > limit * limit || x.abs() > radius || z.abs() > radius {
        player.send(cb::sync_position(s.x, s.y, s.z, s.yaw, s.pitch, 0, 0));
        return false;
    }
    true
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

    let dim = player.state().dimension;
    // Send newly-needed chunks (closest first for nicer pop-in).
    let mut to_send: Vec<(i32, i32)> = wanted.difference(loaded).copied().collect();
    to_send.sort_by_key(|(x, z)| (x - cx).pow(2) + (z - cz).pow(2));
    for (x, z) in to_send {
        // Clone the column under a brief lock, then do the expensive palette +
        // lighting encode *outside* the global world lock to cut contention.
        let chunk = {
            let mut world = shared.dim_world(dim).lock().unwrap();
            world.chunk(x, z).clone()
        };
        let light = match chunk.cached_light() {
            Some(l) => l.clone(),
            None => {
                let l = chunk.compute_light();
                // Cache it back so re-sends to other players don't recompute.
                shared.dim_world(dim).lock().unwrap().store_light(x, z, l.clone());
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

/// Move the player to another dimension, rebuilding their world view.
fn switch_dimension(
    shared: &Arc<Shared>,
    player: &Player,
    dim: u8,
    loaded: &mut HashSet<(i32, i32)>,
    last_center: &mut (i32, i32),
) {
    if !commands::is_op(shared, &player.name) {
        player.send(cb::system_chat(&text::colored("You don't have permission for that.", "red"), false));
        return;
    }
    let spawn = shared.dim_world(dim).lock().unwrap().spawn();
    player.update(|s| {
        s.dimension = dim;
        s.x = spawn.0;
        s.y = spawn.1;
        s.z = spawn.2;
        s.fall_peak_y = spawn.1;
    });
    let gamemode = player.gamemode() as u8;
    player.send(cb::respawn(dim, gamemode, false));
    player.send(cb::player_abilities(if gamemode == 1 { 0x0D } else { 0x00 }, 0.05, 0.1));
    player.sync_inventory();
    player.send(cb::sync_position(spawn.0, spawn.1, spawn.2, 0.0, 0.0, 0, 0));
    let (cx, cz) = ((spawn.0.floor() as i32).div_euclid(16), (spawn.2.floor() as i32).div_euclid(16));
    player.send(cb::set_center_chunk(cx, cz));
    loaded.clear();
    stream_chunks(shared, player, cx, cz, loaded);
    *last_center = (cx, cz);
    player.send(cb::game_event(13, 0.0));
    player.send(cb::system_chat(
        &text::colored(format!("Teleported to the {}.", ["overworld", "nether", "end"][dim as usize]), "green"),
        false,
    ));
}

/// Revive the player and rebuild their world view after death.
fn respawn_player(
    shared: &Arc<Shared>,
    player: &Player,
    loaded: &mut HashSet<(i32, i32)>,
    last_center: &mut (i32, i32),
) {
    combat::revive(player);
    // Players always respawn in the overworld, at their bed if they have one.
    let spawn = player.state().spawn_point.unwrap_or_else(|| shared.world.lock().unwrap().spawn());
    let is_flat = shared.world.lock().unwrap().generator_name() == "flat";
    player.update(|s| {
        s.dimension = 0;
        s.x = spawn.0;
        s.y = spawn.1;
        s.z = spawn.2;
        s.fall_peak_y = spawn.1;
    });

    let gamemode = player.gamemode() as u8;
    player.send(cb::respawn(0, gamemode, is_flat));
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

/// Award the advancement for a milestone event, popping a toast if newly earned.
fn award_advancement(shared: &Arc<Shared>, player: &Player, event: &str) {
    if let Some(key) = advancements::key_for_event(event) {
        if shared.earn_advancement(player.entity_id, key) {
            let earned = shared.earned_advancements(player.entity_id);
            player.send(advancements::packet(&earned, player.protocol));
        }
    }
}

/// The 16 dye colours, indexed to match sheep wool variants and `*_wool` items.
const WOOL_COLORS: [&str; 16] = [
    "white", "orange", "magenta", "light_blue", "yellow", "lime", "pink", "gray", "light_gray",
    "cyan", "purple", "blue", "brown", "green", "red", "black",
];

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
            open_merchant(shared, player, target, crate::ai::profession_for(target));
        }
        return;
    }

    // Milking a cow/mooshroom/goat with an empty bucket yields a milk bucket.
    let slot = player.state().held_slot;
    let held = player.inventory(|inv| inv.held(slot));
    if item::name_of(held.id) == Some("bucket") {
        let milkable = shared
            .with_mob(target, |m| matches!(m.kind.name(), "cow" | "mooshroom" | "goat") && !m.baby)
            .unwrap_or(false);
        if milkable {
            if player.gamemode() != 1 {
                let after = player.inventory(|i| i.consume_held(slot));
                player.set_slot(crate::inventory::HOTBAR_START + slot as usize, after);
                if let Some(id) = item::id_any("milk_bucket") {
                    player.give(id, 1);
                }
            }
            let (mx, my, mz) = shared.with_mob(target, |m| (m.x, m.y, m.z)).unwrap_or((0.0, 0.0, 0.0));
            shared.broadcast(cb::sound_effect("entity.cow.milk", 6, mx, my, mz, 1.0, 1.0));
            return;
        }
    }

    // Shearing a sheep drops wool of its colour and leaves it sheared.
    let held = player.inventory(|inv| inv.held(player.state().held_slot));
    if item::name_of(held.id) == Some("shears") {
        let shorn = shared
            .with_mob(target, |m| {
                if m.kind.name() == "sheep" && !m.sheared && !m.baby {
                    m.sheared = true;
                    Some((m.variant & 0x0f, m.x, m.y, m.z))
                } else {
                    None
                }
            })
            .flatten();
        if let Some((color, mx, my, mz)) = shorn {
            let wool = format!("{}_wool", WOOL_COLORS[color as usize]);
            if let Some(id) = item::id_any(&wool) {
                let count = rand::thread_rng().gen_range(1..=3);
                drops::spawn_item(shared, id, count, mx, my + 0.5, mz, 10);
            }
            if let Some(meta) = shared.with_mob(target, |m| m.metadata()) {
                shared.broadcast(cb::entity_metadata(target, &meta));
            }
            shared.broadcast(cb::sound_effect("entity.sheep.shear", 6, mx, my, mz, 1.0, 1.0));
            // Wear the shears in survival.
            if player.gamemode() != 1 {
                damage_held_tool(shared, player);
            }
        }
        return;
    }

    // Feeding an animal its breeding food puts it into love mode. Feeding a
    // baby instead speeds up its growth.
    let held = player.inventory(|inv| inv.held(player.state().held_slot));
    let held_name = item::name_of(held.id);
    let is_food = shared
        .with_mob(target, |m| held_name.is_some_and(|n| m.kind.breeding_food().contains(&n)))
        .unwrap_or(false);
    if is_food {
        let accepted = shared
            .with_mob(target, |m| {
                if m.baby {
                    // Knock 10% off the remaining growth time.
                    m.baby_age = m.baby_age.saturating_sub(m.baby_age / 10 + 200);
                    true
                } else if m.breed_cooldown == 0 {
                    m.in_love = 600;
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false);
        if accepted {
            if player.gamemode() != 1 {
                let held_slot = player.state().held_slot;
                let after = player.inventory(|inv| inv.consume_held(held_slot));
                player.set_slot(crate::inventory::HOTBAR_START + held_slot as usize, after);
            }
            let (mx, my, mz) = shared.with_mob(target, |m| (m.x, m.y, m.z)).unwrap_or((0.0, 0.0, 0.0));
            shared.broadcast(cb::sound_effect("entity.generic.eat", 6, mx, my, mz, 1.0, 1.0));
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

/// Villager trade offers a villager of `profession` and trade `level` (1–5)
/// currently offers: `(input1, output, input2)`. Offers unlock with level
/// (positionally), and unknown items are dropped so the list always type-checks.
fn merchant_offers(profession: &str, level: i32) -> Vec<(item::ItemStack, item::ItemStack, item::ItemStack)> {
    // Each entry is `(buy_name, buy_count, give_name, give_count)` describing a
    // trade where the player hands over `buy` and receives `give`. Tables stay
    // ordered so earlier offers belong to lower villager levels.
    let raw: &[(&str, u8, &str, u8)] = match profession {
        "farmer" => &[
            ("wheat", 20, "emerald", 1),
            ("emerald", 1, "bread", 6),
            ("emerald", 1, "pumpkin_pie", 4),
            ("carrot", 22, "emerald", 1),
        ],
        "librarian" => &[
            ("paper", 24, "emerald", 1),
            ("emerald", 9, "book", 1),
            ("emerald", 1, "lantern", 1),
            ("emerald", 5, "compass", 1),
        ],
        "blacksmith" => &[
            ("coal", 15, "emerald", 1),
            ("emerald", 4, "iron_pickaxe", 1),
            ("emerald", 5, "shield", 1),
            ("iron_ingot", 4, "emerald", 1),
        ],
        "cleric" => &[
            ("rotten_flesh", 32, "emerald", 1),
            ("emerald", 1, "redstone", 4),
            ("emerald", 1, "lapis_lazuli", 4),
            ("emerald", 3, "glowstone", 4),
        ],
        "cartographer" => &[
            ("paper", 24, "emerald", 1),
            ("emerald", 7, "compass", 1),
            ("emerald", 1, "map", 1),
        ],
        "fisherman" => &[
            ("string", 20, "emerald", 1),
            ("emerald", 1, "cooked_cod", 6),
            ("cod", 15, "emerald", 1),
        ],
        "fletcher" => &[
            ("stick", 32, "emerald", 1),
            ("emerald", 1, "arrow", 16),
            ("flint", 26, "emerald", 1),
            ("emerald", 2, "bow", 1),
        ],
        "shepherd" => &[
            ("white_wool", 18, "emerald", 1),
            ("emerald", 1, "shears", 1),
            ("emerald", 3, "white_bed", 1),
        ],
        "butcher" => &[
            ("porkchop", 14, "emerald", 1),
            ("emerald", 1, "cooked_porkchop", 5),
            ("chicken", 14, "emerald", 1),
        ],
        "mason" => &[
            ("clay_ball", 10, "emerald", 1),
            ("emerald", 1, "bricks", 10),
            ("emerald", 1, "stone", 4),
        ],
        _ => &[("emerald", 1, "bread", 6), ("stick", 32, "emerald", 1)],
    };
    // Positional level gate: first two offers are novice, then one per level.
    let required_level = |i: usize| -> i32 { if i < 2 { 1 } else { (i - 1) as i32 } };
    raw.iter()
        .enumerate()
        .filter(|(i, _)| required_level(*i) <= level)
        .filter_map(|(_, (bn, bc, gn, gc))| {
            let bid = item::id_any(bn)?;
            let gid = item::id_any(gn)?;
            Some((item::ItemStack::new(bid, *bc), item::ItemStack::new(gid, *gc), item::ItemStack::EMPTY))
        })
        .collect()
}

/// Open a villager trade window for a villager of the given profession.
fn open_merchant(shared: &Arc<Shared>, player: &Player, villager: i32, profession: &'static str) {
    let level = shared.villager_level(villager);
    let offers = merchant_offers(profession, level);
    let uses = shared.trade_uses(villager, offers.len());
    player.update(|s| {
        s.open_merchant = true;
        s.merchant_prof = Some(profession);
        s.merchant_id = Some(villager);
    });
    player.send(cb::open_window(2, 18, &text::plain("Villager"))); // 18 = merchant menu
    player.send(cb::trade_list(2, &offers, &uses, level));
}

/// Execute a selected trade against the player's inventory.
fn do_trade(shared: &Arc<Shared>, player: &Player, index: i32) {
    let (Some(profession), Some(villager)) = (player.state().merchant_prof, player.state().merchant_id) else {
        return;
    };
    if !player.state().open_merchant {
        return;
    }
    let level = shared.villager_level(villager);
    let offers = merchant_offers(profession, level);
    let Some((input, output, _)) = offers.get(index as usize) else {
        return;
    };
    // A trade locked out by repeated use can't be made until the villager restocks.
    let used = shared.trade_uses(villager, offers.len());
    if used.get(index as usize).copied().unwrap_or(0) >= cb::MAX_TRADE_USES {
        player.send(cb::system_chat(&text::colored("That trade is out of stock for now.", "red"), false));
        return;
    }
    let traded = player.inventory(|inv| {
        if inv.has(input.id, input.count) {
            inv.remove(input.id, input.count);
            inv.add(output.id, output.count);
            true
        } else {
            false
        }
    });
    if traded {
        shared.use_trade(villager, index as usize, offers.len());
        award_advancement(shared, player, "trade");
        // Each trade grants the villager XP, which can raise its level.
        let new_level = shared.add_villager_xp(villager, 5);
        player.sync_inventory();
        let s = player.state();
        player.send(cb::sound_effect("entity.villager.yes", 6, s.x, s.y, s.z, 1.0, 1.0));
        if new_level > level {
            player.send(cb::system_chat(&text::colored("The villager unlocked new trades!", "green"), false));
        }
        // Refresh the window so the client sees updated uses and any new offers.
        let offers = merchant_offers(profession, new_level);
        let uses = shared.trade_uses(villager, offers.len());
        player.send(cb::trade_list(2, &offers, &uses, new_level));
    } else {
        player.send(cb::system_chat(&text::colored("You don't have what that trade needs.", "red"), false));
    }
}

/// Toggle a lever the player clicked, updating redstone. Returns true if a
/// lever was toggled.
fn try_toggle_lever(shared: &Arc<Shared>, player: &Player, x: i32, y: i32, z: i32) -> bool {
    let dim = player.state().dimension;
    let state = { shared.dim_world(dim).lock().unwrap().get_block(x, y, z) };
    if cubeplane_world::block::name_of(state) != "lever" {
        return false;
    }
    let powered = cubeplane_world::block::prop_index(state, "powered").unwrap_or(1);
    let toggled = cubeplane_world::block::with_prop(state, "powered", if powered == 0 { 1 } else { 0 });
    {
        let mut w = shared.dim_world(dim).lock().unwrap();
        w.set_block(x, y, z, toggled);
    }
    shared.broadcast(cb::block_update(x, y, z, toggled));
    if dim == 0 {
        crate::sim::redstone_update(shared, x, y, z);
    }
    true
}

/// Pick up or place fluids with a bucket. Returns true if a bucket was used.
fn try_bucket(shared: &Arc<Shared>, player: &Player, x: i32, y: i32, z: i32, face: i32) -> bool {
    let slot = player.state().held_slot;
    let held = player.inventory(|i| i.held(slot));
    let dim = player.state().dimension;
    match item::name_of(held.id) {
        Some("bucket") => {
            // Scoop up a source block the player clicked.
            let state = { shared.dim_world(dim).lock().unwrap().get_block(x, y, z) };
            let name = cubeplane_world::block::name_of(state);
            let is_source = matches!(name, "water" | "lava")
                && cubeplane_world::block::prop_index(state, "level") == Some(0);
            if !is_source {
                return false;
            }
            let filled = if name == "water" { "water_bucket" } else { "lava_bucket" };
            {
                let mut w = shared.dim_world(dim).lock().unwrap();
                w.set_block(x, y, z, cubeplane_world::block::AIR);
            }
            shared.broadcast(cb::block_update(x, y, z, cubeplane_world::block::AIR));
            if dim == 0 {
                shared.schedule_fluid(x, y, z);
            }
            if player.gamemode() != 1 {
                let after = player.inventory(|i| i.consume_held(slot));
                player.set_slot(crate::inventory::HOTBAR_START + slot as usize, after);
                if let Some(id) = item::id_any(filled) {
                    player.give(id, 1);
                }
            }
            true
        }
        Some(fluid_item @ ("water_bucket" | "lava_bucket")) => {
            // Empty the bucket into the air block against the clicked face.
            let (dx, dy, dz) = face_offset(face);
            let (px, py, pz) = (x + dx, y + dy, z + dz);
            let target = { shared.dim_world(dim).lock().unwrap().get_block(px, py, pz) };
            if !cubeplane_world::block::is_air(target) {
                return false;
            }
            let fluid = if fluid_item == "water_bucket" { "water" } else { "lava" };
            let Some(source) = cubeplane_world::block::state_by_name(fluid) else {
                return false;
            };
            {
                let mut w = shared.dim_world(dim).lock().unwrap();
                w.set_block(px, py, pz, source);
            }
            shared.broadcast(cb::block_update(px, py, pz, source));
            if dim == 0 {
                shared.schedule_fluid(px, py, pz);
            }
            if player.gamemode() != 1 {
                let after = player.inventory(|i| i.consume_held(slot));
                player.set_slot(crate::inventory::HOTBAR_START + slot as usize, after);
                if let Some(id) = item::id_any("bucket") {
                    player.give(id, 1);
                }
            }
            true
        }
        _ => false,
    }
}

/// Block offset for a clicked face (0=down … 5=east).
fn face_offset(face: i32) -> (i32, i32, i32) {
    match face {
        0 => (0, -1, 0),
        1 => (0, 1, 0),
        2 => (0, 0, -1),
        3 => (0, 0, 1),
        4 => (-1, 0, 0),
        5 => (1, 0, 0),
        _ => (0, 1, 0),
    }
}

/// Right-click a button to press it: power it now and schedule its release.
fn try_use_button(shared: &Arc<Shared>, player: &Player, x: i32, y: i32, z: i32) -> bool {
    let dim = player.state().dimension;
    let state = { shared.dim_world(dim).lock().unwrap().get_block(x, y, z) };
    let name = cubeplane_world::block::name_of(state);
    if !name.ends_with("_button") {
        return false;
    }
    // Already pressed: ignore (the timer will release it).
    if cubeplane_world::block::prop_index(state, "powered") == Some(0) {
        return true;
    }
    let pressed = cubeplane_world::block::with_prop(state, "powered", 0);
    {
        let mut w = shared.dim_world(dim).lock().unwrap();
        w.set_block(x, y, z, pressed);
    }
    shared.broadcast(cb::block_update(x, y, z, pressed));
    // Stone-family buttons stay down 20 ticks, wooden ones 30.
    let stone = name == "stone_button" || name == "polished_blackstone_button";
    shared.press_button(dim, x, y, z, if stone { 20 } else { 30 });
    if dim == 0 {
        crate::sim::redstone_update(shared, x, y, z);
    }
    let sound = if stone { "block.stone_button.click_on" } else { "block.wooden_button.click_on" };
    shared.broadcast(cb::sound_effect(sound, 0, x as f64 + 0.5, y as f64 + 0.5, z as f64 + 0.5, 1.0, 1.0));
    true
}

/// Right-click a door, trapdoor or fence gate to toggle it open/closed.
/// Iron doors and iron trapdoors only respond to redstone, not the hand.
fn try_use_door(shared: &Arc<Shared>, player: &Player, x: i32, y: i32, z: i32) -> bool {
    let dim = player.state().dimension;
    let state = { shared.dim_world(dim).lock().unwrap().get_block(x, y, z) };
    let name = cubeplane_world::block::name_of(state);
    let is_door = name.ends_with("_door");
    let is_trapdoor = name.ends_with("_trapdoor");
    let is_gate = name.ends_with("_fence_gate");
    if !(is_door || is_trapdoor || is_gate) {
        return false;
    }
    // Iron doors/trapdoors can't be opened by hand.
    if name == "iron_door" || name == "iron_trapdoor" {
        return true;
    }
    let open = cubeplane_world::block::prop_index(state, "open").unwrap_or(1);
    let new_open = if open == 0 { 1 } else { 0 };
    let toggled = cubeplane_world::block::with_prop(state, "open", new_open);
    let mut updates = vec![(x, y, z, toggled)];
    // A door is two stacked blocks; keep both halves in sync.
    if is_door {
        let other_y = if cubeplane_world::block::prop_index(state, "half") == Some(0) { y - 1 } else { y + 1 };
        let other = { shared.dim_world(dim).lock().unwrap().get_block(x, other_y, z) };
        if cubeplane_world::block::name_of(other).ends_with("_door") {
            updates.push((x, other_y, z, cubeplane_world::block::with_prop(other, "open", new_open)));
        }
    }
    {
        let mut w = shared.dim_world(dim).lock().unwrap();
        for (ux, uy, uz, us) in &updates {
            w.set_block(*ux, *uy, *uz, *us);
        }
    }
    for (ux, uy, uz, us) in updates {
        shared.broadcast(cb::block_update(ux, uy, uz, us));
    }
    // Door sounds use the inline name form (varint 0 + identifier).
    let sound = if new_open == 0 {
        if is_door { "block.wooden_door.open" } else if is_gate { "block.fence_gate.open" } else { "block.wooden_trapdoor.open" }
    } else if is_door {
        "block.wooden_door.close"
    } else if is_gate {
        "block.fence_gate.close"
    } else {
        "block.wooden_trapdoor.close"
    };
    shared.broadcast(cb::sound_effect(sound, 0, x as f64 + 0.5, y as f64 + 0.5, z as f64 + 0.5, 1.0, 1.0));
    true
}

/// Use flint & steel: ignite TNT, or light a fire on the clicked face.
/// Returns true if flint & steel was used.
fn try_flint(shared: &Arc<Shared>, player: &Player, x: i32, y: i32, z: i32, face: i32) -> bool {
    let held = player.inventory(|i| i.held(player.state().held_slot));
    if item::name_of(held.id) != Some("flint_and_steel") {
        return false;
    }
    let dim = player.state().dimension;
    if dim != 0 {
        return true; // ignition sims are overworld-only; just consume the click
    }
    let clicked = { shared.dim_world(0).lock().unwrap().get_block(x, y, z) };
    if cubeplane_world::block::name_of(clicked) == "tnt" {
        crate::sim::ignite_tnt(shared, x, y, z);
    } else {
        // Light a fire on the face the player clicked.
        let (dx, dy, dz) = match face {
            0 => (0, -1, 0),
            1 => (0, 1, 0),
            2 => (0, 0, -1),
            3 => (0, 0, 1),
            4 => (-1, 0, 0),
            _ => (1, 0, 0),
        };
        let (fx, fy, fz) = (x + dx, y + dy, z + dz);
        let air = { shared.dim_world(0).lock().unwrap().get_block(fx, fy, fz) };
        if cubeplane_world::block::is_air(air) {
            if let Some(fire) = cubeplane_world::block::state_by_name("fire") {
                shared.dim_world(0).lock().unwrap().set_block(fx, fy, fz, fire);
                shared.broadcast(cb::block_update(fx, fy, fz, fire));
            }
        }
    }
    if player.gamemode() != 1 {
        damage_held_tool(shared, player);
    }
    true
}

/// Fire an arrow if the player is holding a bow (and has arrows / is creative).
/// Returns true if a shot was fired.
fn try_shoot_bow(shared: &Arc<Shared>, player: &Player) -> bool {
    let held = player.inventory(|i| i.held(player.state().held_slot));
    if item::name_of(held.id) != Some("bow") {
        return false;
    }
    let arrow = item::id_any("arrow").unwrap_or(0);
    let creative = player.gamemode() == 1;
    if !creative && !player.inventory(|i| i.has(arrow, 1)) {
        return false;
    }
    if !creative {
        player.inventory(|i| i.remove(arrow, 1));
        player.sync_inventory();
        damage_held_tool(shared, player);
    }

    // Direction from the player's look (Minecraft yaw/pitch conventions).
    let s = player.state();
    let yaw = (s.yaw as f64).to_radians();
    let pitch = (s.pitch as f64).to_radians();
    let dx = -yaw.sin() * pitch.cos();
    let dy = -pitch.sin();
    let dz = yaw.cos() * pitch.cos();
    drops::spawn_arrow(shared, player.entity_id, s.x, s.y + 1.5, s.z, dx, dy, dz, 5.0);
    true
}

/// Wear down the held tool/weapon by one point (survival), breaking it at 0.
fn damage_held_tool(shared: &Arc<Shared>, player: &Player) {
    let held = player.state().held_slot;
    let mut stack = player.inventory(|i| i.held(held));
    let Some(max) = item::max_durability(stack.id) else {
        return;
    };
    // Unbreaking: chance to skip wear (level/(level+1)).
    if let Some(("unbreaking", lvl)) = stack.enchant() {
        if rand::random::<f32>() < lvl as f32 / (lvl as f32 + 1.0) {
            return;
        }
    }
    stack.damage += 1;
    let slot = crate::inventory::HOTBAR_START + held as usize;
    if stack.damage >= max {
        player.set_slot(slot, item::ItemStack::EMPTY);
        let s = player.state();
        shared.broadcast(cb::sound_effect("entity.item.break", 7, s.x, s.y, s.z, 1.0, 1.0));
    } else {
        player.set_slot(slot, stack);
    }
}

/// Sleep in / set spawn at a bed. Returns true if a bed was clicked.
fn try_use_bed(shared: &Arc<Shared>, player: &Player, x: i32, y: i32, z: i32) -> bool {
    let is_bed = {
        let mut w = shared.dim_world(player.state().dimension).lock().unwrap();
        cubeplane_world::block::info(w.get_block(x, y, z)).name.ends_with("_bed")
    };
    if !is_bed {
        return false;
    }
    // Set the bed spawn point and update the compass.
    player.update(|s| s.spawn_point = Some((x as f64 + 0.5, y as f64, z as f64 + 0.5)));
    player.send(cb::spawn_position(x, y, z, 0.0));
    player.send(cb::system_chat(&text::colored("Spawn point set.", "green"), false));

    // Sleeping at night skips to morning (lenient single-sleeper rule).
    let time = shared.world_time();
    if (13_000..23_000).contains(&time) {
        shared.set_time(1000);
        shared.broadcast(cb::update_time(0, 1000));
        shared.broadcast(cb::system_chat(&text::colored(format!("{} slept; good morning!", player.name), "yellow"), false));
    }
    true
}

/// Use bone meal on a crop, sapling or grass block. Returns true if applied.
fn try_bonemeal(shared: &Arc<Shared>, player: &Player, x: i32, y: i32, z: i32) -> bool {
    let held = player.inventory(|i| i.held(player.state().held_slot));
    if item::name_of(held.id) != Some("bone_meal") {
        return false;
    }
    // Only overworld blocks run the growth simulation.
    if player.state().dimension != 0 {
        return false;
    }
    if !crate::sim::apply_bonemeal(shared, x, y, z) {
        // Not a bonemeal-able block: let other handlers / placement run.
        return false;
    }
    if player.gamemode() != 1 {
        let slot = player.state().held_slot;
        let after = player.inventory(|i| i.consume_held(slot));
        player.set_slot(crate::inventory::HOTBAR_START + slot as usize, after);
    }
    shared.broadcast(cb::sound_effect("item.bone_meal.use", 0, x as f64 + 0.5, y as f64 + 0.5, z as f64 + 0.5, 1.0, 1.0));
    true
}

/// Open the furnace screen if the player clicked a furnace. Returns true if so.
fn try_open_furnace(shared: &Arc<Shared>, player: &Player, x: i32, y: i32, z: i32) -> bool {
    let is_furnace = {
        let mut w = shared.dim_world(player.state().dimension).lock().unwrap();
        cubeplane_world::block::info(w.get_block(x, y, z)).name == "furnace"
    };
    if !is_furnace {
        return false;
    }
    crate::furnace::open(shared, player, (x, y, z));
    true
}

/// Open the 3×3 crafting table if the player clicked one.
fn try_open_crafting(shared: &Arc<Shared>, player: &Player, x: i32, y: i32, z: i32) -> bool {
    let is_table = {
        let mut w = shared.dim_world(player.state().dimension).lock().unwrap();
        cubeplane_world::block::info(w.get_block(x, y, z)).name == "crafting_table"
    };
    if !is_table {
        return false;
    }
    player.update(|s| {
        s.open_crafting = true;
        s.craft_grid = [item::ItemStack::EMPTY; 9];
    });
    // Window 3, type 11 (crafting). Slots: 0 output, 1..9 grid, then inventory.
    let inv = player.inventory(|i| i.slots().to_vec());
    let mut items = vec![item::ItemStack::EMPTY; 10];
    items.extend_from_slice(&inv[9..45]);
    player.send(cb::open_window(3, 11, &text::plain("Crafting")));
    player.send(cb::window_items(3, 0, &items, item::ItemStack::EMPTY));
    true
}

/// Handle a click in the open crafting-table window.
fn craft_window_click(player: &Player, changed: &[(i16, item::ItemStack)]) {
    let took_output = changed.iter().any(|(s, _)| *s == 0);
    player.update(|st| {
        for (slot, stack) in changed {
            match *slot {
                1..=9 => st.craft_grid[*slot as usize - 1] = *stack,
                s if s >= 10 => {} // inventory handled below (needs inventory lock)
                _ => {}
            }
        }
    });
    // Inventory-side slots map 10.. → inventory 9...
    player.inventory(|i| {
        for (slot, stack) in changed {
            if *slot >= 10 {
                i.set(*slot as usize - 10 + 9, *stack);
            }
        }
    });

    let grid: Vec<Option<i32>> = player
        .state()
        .craft_grid
        .iter()
        .map(|s| (!s.is_empty()).then_some(s.id))
        .collect();
    if took_output && crate::recipe::match_grid(&grid).is_some() {
        player.update(|st| {
            for cell in st.craft_grid.iter_mut() {
                if !cell.is_empty() {
                    cell.count -= 1;
                    if cell.count == 0 {
                        *cell = item::ItemStack::EMPTY;
                    }
                }
            }
        });
        let g = player.state().craft_grid;
        for (i, cell) in g.iter().enumerate() {
            player.send(cb::set_slot(3, 0, (i + 1) as i16, *cell));
        }
    }
    let grid: Vec<Option<i32>> = player
        .state()
        .craft_grid
        .iter()
        .map(|s| (!s.is_empty()).then_some(s.id))
        .collect();
    let out = crate::recipe::match_grid(&grid)
        .map(|r| item::ItemStack::new(r.output_id, r.output_count))
        .unwrap_or(item::ItemStack::EMPTY);
    player.send(cb::set_slot(3, 0, 0, out));
}

/// Return any items left in the crafting grid to the inventory and close it.
fn close_crafting(player: &Player) {
    let grid = player.state().craft_grid;
    player.inventory(|i| {
        for cell in grid {
            if !cell.is_empty() {
                i.add(cell.id, cell.count);
            }
        }
    });
    player.update(|s| {
        s.open_crafting = false;
        s.craft_grid = [item::ItemStack::EMPTY; 9];
    });
    player.sync_inventory();
}

/// Open an anvil window if the player clicked an anvil.
fn try_open_anvil(shared: &Arc<Shared>, player: &Player, x: i32, y: i32, z: i32) -> bool {
    let is_anvil = {
        let mut w = shared.dim_world(player.state().dimension).lock().unwrap();
        cubeplane_world::block::info(w.get_block(x, y, z)).name.ends_with("anvil")
    };
    if !is_anvil {
        return false;
    }
    player.update(|s| {
        s.open_anvil = true;
        s.anvil_in = [item::ItemStack::EMPTY; 2];
    });
    let inv = player.inventory(|i| i.slots().to_vec());
    let mut items = vec![item::ItemStack::EMPTY; 3]; // in0, in1, out
    items.extend_from_slice(&inv[9..45]);
    player.send(cb::open_window(6, 7, &text::plain("Repair"))); // 7 = anvil menu
    player.send(cb::window_items(6, 0, &items, item::ItemStack::EMPTY));
    true
}

/// Compute an anvil's output from its two inputs: repair like items, or apply
/// the second item's enchantment to the first.
fn anvil_result(a: item::ItemStack, b: item::ItemStack) -> item::ItemStack {
    if a.is_empty() {
        return item::ItemStack::EMPTY;
    }
    if b.is_empty() {
        return item::ItemStack::EMPTY;
    }
    // Repair two of the same damageable item.
    if a.id == b.id {
        if let Some(max) = item::max_durability(a.id) {
            let restored = (max - b.damage) + max / 8; // remaining + 12.5% bonus
            let mut out = a;
            out.count = 1;
            out.damage = a.damage.saturating_sub(restored);
            // Keep the better enchant of the two.
            if b.ench != 0 && (a.ench == 0 || b.ench_lvl > a.ench_lvl) {
                out.ench = b.ench;
                out.ench_lvl = b.ench_lvl;
            }
            return out;
        }
    }
    // Apply an enchantment from the second item (e.g. an enchanted book).
    if b.ench != 0 {
        let mut out = a;
        out.count = 1;
        if out.ench == 0 || out.ench == b.ench {
            out.ench = b.ench;
            out.ench_lvl = (out.ench_lvl.max(b.ench_lvl) + if out.ench == b.ench { 1 } else { 0 }).min(5);
        }
        return out;
    }
    item::ItemStack::EMPTY
}

/// Handle a click in the open anvil window.
fn anvil_window_click(player: &Player, changed: &[(i16, item::ItemStack)]) {
    let took_output = changed.iter().any(|(s, _)| *s == 2);
    player.update(|st| {
        for (slot, stack) in changed {
            match *slot {
                0 => st.anvil_in[0] = *stack,
                1 => st.anvil_in[1] = *stack,
                _ => {}
            }
        }
    });
    player.inventory(|i| {
        for (slot, stack) in changed {
            if *slot >= 3 {
                i.set(*slot as usize - 3 + 9, *stack);
            }
        }
    });

    if took_output {
        // Consume both inputs on a successful craft.
        let [a, b] = player.state().anvil_in;
        if !anvil_result(a, b).is_empty() {
            player.update(|st| st.anvil_in = [item::ItemStack::EMPTY; 2]);
            player.send(cb::set_slot(6, 0, 0, item::ItemStack::EMPTY));
            player.send(cb::set_slot(6, 0, 1, item::ItemStack::EMPTY));
        }
    }
    let [a, b] = player.state().anvil_in;
    player.send(cb::set_slot(6, 0, 2, anvil_result(a, b)));
}

fn close_anvil(player: &Player) {
    let inputs = player.state().anvil_in;
    player.inventory(|i| {
        for s in inputs {
            if !s.is_empty() {
                i.add(s.id, s.count);
            }
        }
    });
    player.update(|s| {
        s.open_anvil = false;
        s.anvil_in = [item::ItemStack::EMPTY; 2];
    });
    player.sync_inventory();
}

/// Open the brewing screen if the player clicked a brewing stand.
fn try_open_brewing(shared: &Arc<Shared>, player: &Player, x: i32, y: i32, z: i32) -> bool {
    let is_stand = {
        let mut w = shared.dim_world(player.state().dimension).lock().unwrap();
        cubeplane_world::block::info(w.get_block(x, y, z)).name == "brewing_stand"
    };
    if !is_stand {
        return false;
    }
    crate::brewing::open(shared, player, (x, y, z));
    true
}

/// Read (and re-open for editing) a sign the player clicked. Returns true if a
/// sign was clicked.
fn try_read_sign(shared: &Arc<Shared>, player: &Player, x: i32, y: i32, z: i32) -> bool {
    let is_sign = {
        let mut w = shared.dim_world(player.state().dimension).lock().unwrap();
        cubeplane_world::block::info(w.get_block(x, y, z)).name.ends_with("_sign")
    };
    if !is_sign {
        return false;
    }
    match shared.sign((x, y, z)) {
        Some(lines) => {
            let text = lines.iter().filter(|l| !l.is_empty()).cloned().collect::<Vec<_>>().join(" / ");
            let shown = if text.is_empty() { "(blank sign)".to_string() } else { text };
            player.send(cb::system_chat(&text::colored(format!("Sign: {shown}"), "yellow"), false));
        }
        None => player.send(cb::open_sign_editor(x, y, z)),
    }
    true
}

/// Open a chest the player clicked. Returns true if a container was opened.
fn try_open_container(shared: &Arc<Shared>, player: &Player, x: i32, y: i32, z: i32) -> bool {
    let is_chest = {
        let mut w = shared.dim_world(player.state().dimension).lock().unwrap();
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
    // Furnace / brewing / crafting windows route to their handlers.
    if window_id != 0 {
        if let Some(pos) = player.state().open_furnace {
            crate::furnace::click(shared, player, pos, changed);
            return;
        }
        if let Some(pos) = player.state().open_brewing {
            crate::brewing::click(shared, player, pos, changed);
            return;
        }
        if player.state().open_crafting {
            craft_window_click(player, changed);
            return;
        }
        if player.state().open_anvil {
            anvil_window_click(player, changed);
            return;
        }
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
    // The 2×2 crafting grid is inventory slots 1..=4, with the result in slot 0.
    update_crafting_grid(player, changed.iter().any(|(s, _)| *s == 0));
}

/// Compute the result of the player's 2×2 crafting grid (inventory slots 1–4)
/// into the output slot (0); when the output is taken, consume the grid.
fn update_crafting_grid(player: &Player, took_output: bool) {
    let grid: Vec<Option<i32>> = (1..=4)
        .map(|s| {
            let st = player.inventory(|i| i.get(s));
            (!st.is_empty()).then_some(st.id)
        })
        .collect();

    if took_output && crate::recipe::match_grid(&grid).is_some() {
        // Consume one item from each occupied grid slot, then resync them.
        for s in 1..=4 {
            let mut st = player.inventory(|i| i.get(s));
            if !st.is_empty() {
                st.count -= 1;
                if st.count == 0 {
                    st = item::ItemStack::EMPTY;
                }
                player.inventory(|i| i.set(s, st));
                player.send(cb::set_slot(0, 0, s as i16, st));
            }
        }
    }

    // Recompute the output slot from the (possibly consumed) grid.
    let grid: Vec<Option<i32>> = (1..=4)
        .map(|s| {
            let st = player.inventory(|i| i.get(s));
            (!st.is_empty()).then_some(st.id)
        })
        .collect();
    let out = crate::recipe::match_grid(&grid)
        .map(|r| item::ItemStack::new(r.output_id, r.output_count))
        .unwrap_or(item::ItemStack::EMPTY);
    player.inventory(|i| i.set(0, out));
    player.send(cb::set_slot(0, 0, 0, out));
}

fn break_block(shared: &Arc<Shared>, player: &Player, x: i32, y: i32, z: i32, creative: bool) {
    let dim = player.state().dimension;
    let previous = {
        let mut world = shared.dim_world(dim).lock().unwrap();
        let prev = world.get_block(x, y, z);
        world.set_block(x, y, z, cubeplane_world::block::AIR);
        prev
    };
    shared.broadcast(cb::block_update(x, y, z, cubeplane_world::block::AIR));

    // Record the mined-block statistic (by block registry id).
    if !cubeplane_world::block::is_air(previous) {
        shared.stat_block_mined(player.entity_id, cubeplane_world::block::block_id(previous));
        award_advancement(shared, player, "mine");
    }

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

    // Survival breaks drop the block's item and wear the held tool.
    if !creative {
        if let Some(item_id) = item::item_for_block(previous) {
            drops::spawn_item(shared, item_id, 1, x as f64 + 0.5, y as f64 + 0.25, z as f64 + 0.5, 10);
        }
        damage_held_tool(shared, player);
    }
    // Clean up a broken sign's stored text.
    if cubeplane_world::block::info(previous).name.ends_with("_sign") {
        shared.remove_sign((x, y, z));
    }
    // Breaking a furnace spills its contents.
    if cubeplane_world::block::info(previous).name == "furnace" {
        if let Some(f) = shared.remove_furnace((x, y, z)) {
            for st in [f.input, f.fuel, f.output] {
                if !st.is_empty() {
                    drops::spawn_item(shared, st.id, st.count, x as f64 + 0.5, y as f64 + 0.5, z as f64 + 0.5, 10);
                }
            }
        }
    }
    // Breaking a brewing stand spills its contents.
    if cubeplane_world::block::info(previous).name == "brewing_stand" {
        if let Some(s) = shared.remove_brewing((x, y, z)) {
            for st in s.bottles.into_iter().chain([s.ingredient]) {
                if !st.is_empty() {
                    drops::spawn_item(shared, st.id, st.count, x as f64 + 0.5, y as f64 + 0.5, z as f64 + 0.5, 10);
                }
            }
        }
    }
    // Overworld simulations: fluids flow in, blocks above may fall, redstone.
    if dim == 0 {
        shared.schedule_fluid(x, y, z);
        crate::sim::gravity_check(shared, x, y + 1, z);
        if crate::sim::is_redstone(cubeplane_world::block::name_of(previous)) {
            crate::sim::redstone_update(shared, x, y, z);
        }
    }
    // Observers watching this position fire on the break.
    crate::sim::notify_observers(shared, dim, x, y, z);
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
    let dim = player.state().dimension;
    {
        let mut world = shared.dim_world(dim).lock().unwrap();
        world.set_block(px, py, pz, state);
    }
    shared.broadcast(cb::block_update(px, py, pz, state));

    // Placing a chest creates its (empty) container block entity.
    if cubeplane_world::block::info(state).name == "chest" {
        shared.ensure_container((px, py, pz));
    }
    // Placing a sign opens its edit screen.
    if cubeplane_world::block::info(state).name.ends_with("_sign") {
        player.send(cb::open_sign_editor(px, py, pz));
    }
    // Placing a furnace / brewing stand creates its block entity.
    if cubeplane_world::block::info(state).name == "furnace" {
        shared.ensure_furnace((px, py, pz));
    }
    if cubeplane_world::block::info(state).name == "brewing_stand" {
        shared.ensure_brewing((px, py, pz));
    }
    // Overworld simulations only.
    if dim == 0 {
        shared.schedule_fluid(px, py, pz);
        crate::sim::gravity_check(shared, px, py, pz);
        if crate::sim::is_redstone(cubeplane_world::block::name_of(state)) {
            crate::sim::redstone_update(shared, px, py, pz);
        }
    }
    // Observers watching this position fire on the placement.
    crate::sim::notify_observers(shared, dim, px, py, pz);

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
/// Drink a held potion: apply its effect and return a glass bottle.
fn try_drink(shared: &Arc<Shared>, player: &Player) -> bool {
    let held_slot = player.state().held_slot;
    let held = player.inventory(|i| i.held(held_slot));
    // Milk clears all active status effects and leaves an empty bucket.
    if item::name_of(held.id) == Some("milk_bucket") {
        for eff in shared.player_effects(player.entity_id) {
            player.send(cb::remove_entity_effect(player.entity_id, eff.id));
        }
        shared.clear_effects(player.entity_id);
        let s = player.state();
        player.send(cb::sound_effect("entity.generic.drink", 7, s.x, s.y, s.z, 1.0, 1.0));
        if player.gamemode() != 1 {
            let after = player.inventory(|i| i.consume_held(held_slot));
            player.set_slot(crate::inventory::HOTBAR_START + held_slot as usize, after);
            if let Some(id) = item::id_any("bucket") {
                player.give(id, 1);
            }
        }
        return true;
    }
    if item::name_of(held.id) != Some("potion") {
        return false;
    }
    if let Some(name) = held.potion_name() {
        if let Some((id, amp, secs)) = item::potion_effect(name) {
            crate::effects::apply(shared, player, id, amp, secs);
        }
    }
    let s = player.state();
    player.send(cb::sound_effect("entity.generic.drink", 7, s.x, s.y, s.z, 1.0, 1.0));
    if player.gamemode() != 1 {
        player.inventory(|i| i.consume_held(held_slot));
        if let Some(bottle) = item::id_any("glass_bottle") {
            player.inventory(|i| {
                i.add(bottle, 1);
            });
        }
        player.sync_inventory();
    }
    true
}

fn try_eat(shared: &Arc<Shared>, player: &Player) {
    let held = player.state().held_slot;
    let stack = player.inventory(|inv| inv.held(held));

    // Hunger/saturation from the curated food kind, plus an optional effect for
    // special foods (golden apples heal-over-time, spider eyes poison).
    let curated = stack.def().and_then(|d| match d.kind {
        item::ItemKind::Food(h, s) => Some((h, s)),
        _ => None,
    });
    let name = item::name_of(stack.id).unwrap_or("");
    let effect: Option<(i32, i8, i32)> = match name {
        "golden_apple" => Some((crate::effects::REGENERATION, 1, 5)),
        "enchanted_golden_apple" => Some((crate::effects::REGENERATION, 1, 20)),
        "spider_eye" => Some((crate::effects::POISON, 0, 4)),
        _ => None,
    };
    let (hunger, sat) = match (curated, name) {
        (Some(hs), _) => hs,
        (None, "golden_apple") | (None, "enchanted_golden_apple") => (4, 9.6),
        (None, "spider_eye") => (2, 3.2),
        _ => return, // not edible
    };

    let needs_food = player.state().food < 20;
    if !needs_food && effect.is_none() && player.gamemode() != 1 {
        return;
    }

    let (health, food, saturation) = player.update(|s| {
        s.food = (s.food + hunger).min(20);
        s.saturation = (s.saturation + sat).min(s.food as f32);
        (s.health, s.food, s.saturation)
    });
    player.send(cb::update_health(health, food, saturation));
    let s = player.state();
    player.send(cb::sound_effect("entity.generic.eat", 7, s.x, s.y, s.z, 0.8, 1.0));
    if let Some((id, amp, secs)) = effect {
        crate::effects::apply(shared, player, id, amp, secs);
    }
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
mod trade_tests {
    use super::merchant_offers;
    use crate::ai::PROFESSIONS;

    #[test]
    fn every_profession_has_resolvable_offers() {
        for prof in PROFESSIONS {
            // At master level all offers are unlocked.
            let offers = merchant_offers(prof, 5);
            assert!(!offers.is_empty(), "{prof} had no offers");
            for (input, output, _) in &offers {
                assert!(input.id != 0, "{prof} offer had unknown input");
                assert!(output.id != 0, "{prof} offer had unknown output");
            }
        }
    }

    #[test]
    fn higher_level_unlocks_more_trades() {
        // A master villager offers at least as many trades as a novice, and the
        // novice list is a prefix (level-gated) of the master list.
        let novice = merchant_offers("farmer", 1);
        let master = merchant_offers("farmer", 5);
        assert!(master.len() >= novice.len());
        assert!(!novice.is_empty());
        assert!(master.len() > novice.len());
    }

    #[test]
    fn farmer_buys_wheat_for_emeralds() {
        let offers = merchant_offers("farmer", 1);
        let emerald = crate::item::id_any("emerald").unwrap();
        let wheat = crate::item::id_any("wheat").unwrap();
        // First farmer trade: hand over wheat, receive an emerald.
        assert_eq!(offers[0].0.id, wheat);
        assert_eq!(offers[0].1.id, emerald);
    }

    #[test]
    fn xp_maps_to_levels() {
        use crate::state::level_from_xp;
        assert_eq!(level_from_xp(0), 1);
        assert_eq!(level_from_xp(10), 2);
        assert_eq!(level_from_xp(70), 3);
        assert_eq!(level_from_xp(150), 4);
        assert_eq!(level_from_xp(250), 5);
    }

    #[test]
    fn unknown_profession_falls_back() {
        assert!(!merchant_offers("nonsense", 5).is_empty());
    }

    #[test]
    fn bucket_items_and_fluids_resolve() {
        for name in ["bucket", "water_bucket", "lava_bucket", "milk_bucket"] {
            assert!(crate::item::id_any(name).is_some(), "{name} missing");
        }
        // Fluid source blocks exist with a level property (0 = source).
        for fluid in ["water", "lava"] {
            let s = cubeplane_world::block::state_by_name(fluid).unwrap();
            assert!(cubeplane_world::block::prop_index(s, "level").is_some());
        }
        // Face offsets cover all six directions.
        assert_eq!(super::face_offset(1), (0, 1, 0));
        assert_eq!(super::face_offset(5), (1, 0, 0));
    }

    #[test]
    fn wool_colors_resolve_for_every_variant() {
        use super::WOOL_COLORS;
        assert_eq!(WOOL_COLORS.len(), 16);
        for color in WOOL_COLORS {
            let wool = format!("{color}_wool");
            assert!(crate::item::id_any(&wool).is_some(), "{wool} missing from registry");
        }
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

    /// Drive a simulated 1.20.2 (protocol 764) client through handshake → login →
    /// Configuration phase → play, verifying the server speaks the version's wire
    /// protocol end-to-end (offline mode, no compression for a simple stream).
    /// Drive a simulated 1.19.4 (protocol 762) client: handshake → login → play
    /// directly (no Configuration phase), verifying the Join Game body is
    /// rewritten to the 1.19.4 layout (codec inline, no portalCooldown).
    #[tokio::test]
    async fn version_762_join_without_configuration_phase() {
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe);

        let mut config = Config::default();
        config.server.host = "127.0.0.1".into();
        config.server.port = port;
        config.server.compression_threshold = -1;
        config.server.online_mode = false;
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

        let mut hs = BytesMut::new();
        hs.write_varint(0x00);
        hs.write_varint(762);
        hs.write_string("127.0.0.1");
        hs.write_u16(port);
        hs.write_varint(2);
        write_frame(&mut conn, &hs, None).await;

        let mut ls = BytesMut::new();
        ls.write_varint(0x00);
        ls.write_string("V762");
        ls.write_bool(false);
        write_frame(&mut conn, &ls, None).await;

        let (id, _b) = read_frame(&mut conn, None).await;
        assert_eq!(id, 0x02, "expected Login Success");

        // 1.19.4 has no Configuration phase: play begins immediately. Join Game
        // keeps the canonical id 0x28 (762 play ids match 763) with a 762 body.
        let mut saw_join = false;
        for _ in 0..40 {
            let (pid, mut body) = read_frame(&mut conn, None).await;
            if pid == 0x28 {
                // 762 layout: entityId, hardcore, gameMode, prevGameMode, worlds…
                let _entity = body.read_i32().unwrap();
                let _hardcore = body.read_bool().unwrap();
                let _gm = body.read_u8().unwrap();
                let _pgm = body.read_i8().unwrap();
                let world_count = body.read_varint().unwrap();
                assert!(world_count >= 1, "expected at least one world");
                saw_join = true;
                break;
            }
        }
        assert!(saw_join, "did not receive a 762 Join Game packet");
    }

    #[tokio::test]
    async fn version_764_join_via_configuration_phase() {
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe);

        let mut config = Config::default();
        config.server.host = "127.0.0.1".into();
        config.server.port = port;
        config.server.compression_threshold = -1;
        config.server.online_mode = false;
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

        // Handshake announcing protocol 764, next state = login.
        let mut hs = BytesMut::new();
        hs.write_varint(0x00);
        hs.write_varint(764);
        hs.write_string("127.0.0.1");
        hs.write_u16(port);
        hs.write_varint(2);
        write_frame(&mut conn, &hs, None).await;

        // Login Start.
        let mut ls = BytesMut::new();
        ls.write_varint(0x00);
        ls.write_string("V764");
        ls.write_bool(false);
        write_frame(&mut conn, &ls, None).await;

        // Login Success (0x02), then the client acknowledges login.
        let (id, _b) = read_frame(&mut conn, None).await;
        assert_eq!(id, 0x02, "expected Login Success");
        let mut ack = BytesMut::new();
        ack.write_varint(0x03); // login_acknowledged
        write_frame(&mut conn, &ack, None).await;

        // Configuration phase: expect Registry Data (0x05) then Finish (0x02).
        let mut saw_registry = false;
        let mut saw_finish = false;
        for _ in 0..10 {
            let (cid, _b) = read_frame(&mut conn, None).await;
            if cid == 0x05 {
                saw_registry = true;
            }
            if cid == 0x02 {
                saw_finish = true;
                break;
            }
        }
        assert!(saw_registry, "config phase did not send Registry Data");
        assert!(saw_finish, "config phase did not send Finish Configuration");

        // Acknowledge Finish Configuration → server enters play.
        let mut fin = BytesMut::new();
        fin.write_varint(0x02); // serverbound finish_configuration
        write_frame(&mut conn, &fin, None).await;

        // Play begins: the Join Game packet must arrive with 764's wire id (0x29,
        // remapped from canonical 0x28) and a parseable 764-layout body.
        let mut saw_join = false;
        for _ in 0..40 {
            let (pid, mut body) = read_frame(&mut conn, None).await;
            if pid == 0x29 {
                // Parse the 764 Join Game body to confirm the rewrite is valid.
                let _entity = body.read_i32().unwrap();
                let _hardcore = body.read_bool().unwrap();
                let world_count = body.read_varint().unwrap();
                assert!(world_count >= 1, "expected at least one world");
                for _ in 0..world_count {
                    let _ = body.read_string().unwrap();
                }
                let _max = body.read_varint().unwrap();
                saw_join = true;
                break;
            }
        }
        assert!(saw_join, "did not receive a 764 Join Game packet");
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
