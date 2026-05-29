//! # cubeplane-server
//!
//! The networking and gameplay engine: it accepts TCP connections, drives the
//! handshake → status/login → play state machine, streams the world, relays
//! players to one another, bridges the JS mod runtime and exposes the control
//! API consumed by the Atlas admin panel.

mod ai;
mod clientbound;
mod codec;
mod combat;
mod commands;
mod config;
mod connection;
mod control;
mod drops;
mod encryption;
mod entity;
mod ids;
mod inventory;
mod item;
mod items_table;
mod mobs;
mod mobs_table;
mod mod_actions;
mod persistence;
mod player;
mod registry;
mod serverbound;
mod sim;
mod state;
mod text;

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::net::TcpListener;
use tracing::{error, info};

use cubeplane_mods::{ModEvent, ModRuntime};
use cubeplane_protocol::{GAME_VERSION, PROTOCOL_VERSION};
use cubeplane_world::{FlatGenerator, Generator, TerrainGenerator, World};

use crate::entity::{ItemEntity, Mob, MobKind, Vehicle};

pub use config::Config;
pub use state::Shared;

/// Build the world generator named in the config.
fn make_generator(config: &Config) -> Arc<dyn Generator> {
    match config.world.generator.as_str() {
        "flat" => Arc::new(FlatGenerator::default()),
        _ => Arc::new(TerrainGenerator {
            seed: config.world.seed,
            ..Default::default()
        }),
    }
}

/// Boot the server with the given configuration and run until the process is
/// terminated.
pub async fn run(config: Config) -> Result<()> {
    let mut world = World::new(make_generator(&config));

    // Load persisted world data in the configured format.
    let save_dir = std::path::PathBuf::from(&config.world.save_dir);
    let region = config.world.format == "region";
    if config.world.save {
        if region {
            // Full-chunk backend: install a loader that reads saved chunks.
            let store = persistence::RegionStore::new(&save_dir);
            world.set_loader(Box::new(move |cx, cz| store.load_chunk(cx, cz)));
            info!("persistence: region (full-chunk) backend");
        } else {
            let edits = persistence::load_blocks(&save_dir);
            if !edits.is_empty() {
                info!("loaded {} saved block edit(s)", edits.len());
            }
            world.load_edits(edits);
        }
    }

    // Start the mod runtime, if enabled.
    let (mods, action_rx) = if config.mods.enabled {
        let (rt, rx) = ModRuntime::spawn(&config.mods.dir);
        info!("mods: discovered {} ({:?})", rt.loaded().len(), rt.loaded());
        (Some(rt), Some(rx))
    } else {
        (None, None)
    };

    // Generate an RSA keypair for online mode.
    let server_key = if config.server.online_mode {
        match encryption::ServerKey::generate() {
            Ok(k) => {
                info!("online mode: RSA keypair generated");
                Some(Arc::new(k))
            }
            Err(e) => {
                error!("failed to generate server key, falling back to offline: {e}");
                None
            }
        }
    } else {
        None
    };

    let shared = Shared::new(config, world, mods, server_key);

    // Restore saved chest contents.
    if shared.config.world.save {
        let entries = persistence::load_containers(&save_dir);
        if !entries.is_empty() {
            let map = entries
                .into_iter()
                .map(|(pos, items)| {
                    let stacks = items
                        .into_iter()
                        .map(|(id, count)| crate::item::ItemStack::new(id, count))
                        .collect();
                    (pos, stacks)
                })
                .collect();
            shared.load_containers(map);
        }
        // Restore signs.
        let signs = persistence::load_signs(&save_dir);
        if !signs.is_empty() {
            shared.load_signs(signs.into_iter().collect());
        }
        // Restore world clock and live entities.
        let meta = persistence::load_meta(&save_dir);
        if meta.time != 0 {
            shared.set_time(meta.time);
        }
        let ents = persistence::load_entities(&save_dir);
        let n = ents.mobs.len() + ents.vehicles.len() + ents.items.len();
        restore_entities(&shared, ents);
        if n > 0 {
            info!("restored {n} saved entit(ies)");
        }
    }

    shared.fire_mod(ModEvent::ServerStart {
        version: GAME_VERSION.to_string(),
    });

    if let Some(rx) = action_rx {
        tokio::spawn(mod_actions::run(shared.clone(), rx));
    }

    tokio::spawn(game_loop(shared.clone()));

    if shared.config.world.save {
        tokio::spawn(save_loop(shared.clone(), save_dir.clone(), region));
    }

    if shared.config.control.enabled {
        let s = shared.clone();
        tokio::spawn(async move {
            if let Err(e) = control::serve(s).await {
                error!("control API stopped: {e}");
            }
        });
    }

    let addr = format!(
        "{}:{}",
        shared.config.server.host, shared.config.server.port
    );
    let listener = TcpListener::bind(&addr).await?;
    info!("cubeplane listening on {addr} — Minecraft {GAME_VERSION} (protocol {PROTOCOL_VERSION})");

    // Accept connections until Ctrl-C, then flush a final save.
    let accept = async {
        loop {
            match listener.accept().await {
                Ok((stream, _peer)) => {
                    let s = shared.clone();
                    tokio::spawn(connection::handle(stream, s));
                }
                Err(e) => {
                    error!("accept failed: {e}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    };
    tokio::select! {
        _ = accept => {}
        _ = tokio::signal::ctrl_c() => {
            info!("shutting down — saving world…");
            shared.fire_mod(ModEvent::ServerStop);
            if let Some(m) = &shared.mods {
                m.shutdown();
            }
            if shared.config.world.save {
                save_all(&shared, &save_dir, region);
            }
        }
    }
    Ok(())
}

/// Persist everything once: world (blocks or region), containers, players,
/// live entities and the world clock.
fn save_all(shared: &Arc<Shared>, save_dir: &std::path::Path, region: bool) {
    if region {
        let store = persistence::RegionStore::new(save_dir);
        for ((cx, cz), grid) in shared.world.lock().unwrap().take_dirty_grids() {
            let _ = store.save_chunk(cx, cz, &grid);
        }
    } else {
        let edits = shared.world.lock().unwrap().edits().clone();
        let _ = persistence::save_blocks(save_dir, &edits);
    }

    let containers: Vec<_> = shared
        .containers_snapshot()
        .into_iter()
        .map(|(pos, stacks)| (pos, stacks.iter().map(|s| (s.id, s.count)).collect::<Vec<_>>()))
        .collect();
    let _ = persistence::save_containers(save_dir, &containers);

    for player in shared.players() {
        let _ = persistence::save_player(save_dir, player.uuid, &player.snapshot_data());
    }

    let signs: Vec<_> = shared.signs_snapshot().into_iter().collect();
    let _ = persistence::save_signs(save_dir, &signs);

    let _ = persistence::save_entities(save_dir, &snapshot_entities(shared));
    let _ = persistence::save_meta(save_dir, &persistence::WorldMeta { time: shared.world_time() });
}

/// Capture live mobs, vehicles and dropped items for persistence.
fn snapshot_entities(shared: &Arc<Shared>) -> persistence::EntitySave {
    persistence::EntitySave {
        mobs: shared
            .mobs()
            .iter()
            .filter(|m| m.alive())
            .map(|m| (m.kind.name().to_string(), m.x, m.y, m.z, m.yaw, m.health))
            .collect(),
        vehicles: shared.vehicles().iter().map(|v| (v.type_id, v.x, v.y, v.z, v.yaw)).collect(),
        items: shared
            .item_entities()
            .iter()
            .map(|i| (i.item_id, i.count, i.x, i.y, i.z))
            .collect(),
    }
}

/// Recreate saved mobs, vehicles and items at startup.
fn restore_entities(shared: &Arc<Shared>, ents: persistence::EntitySave) {
    for (name, x, y, z, yaw, health) in ents.mobs {
        if let Some(kind) = MobKind::from_name(&name) {
            let eid = shared.next_entity_id();
            let mut mob = Mob::new(eid, kind, x, y, z, yaw.to_radians());
            mob.health = health;
            mob.yaw = yaw;
            shared.add_mob(mob);
            if kind.name() == "villager" {
                shared.register_villager(eid);
            }
        }
    }
    for (type_id, x, y, z, yaw) in ents.vehicles {
        let eid = shared.next_entity_id();
        shared.add_vehicle(Vehicle { entity_id: eid, type_id, uuid: uuid::Uuid::new_v4(), x, y, z, yaw, rider: None });
    }
    for (item_id, count, x, y, z) in ents.items {
        let eid = shared.next_entity_id();
        shared.add_item_entity(ItemEntity {
            entity_id: eid,
            uuid: uuid::Uuid::new_v4(),
            item_id,
            count,
            x,
            y,
            z,
            on_ground: true,
            age: 0,
            pickup_delay: 0,
        });
    }
}

/// The 20 TPS server clock: drives mod ticks, keep-alives and world time.
async fn game_loop(shared: Arc<Shared>) {
    let mut interval = tokio::time::interval(Duration::from_millis(50));
    let mut ticks: u64 = 0;
    loop {
        interval.tick().await;
        ticks += 1;

        // Advance the world clock; mobs, items and projectiles run every tick.
        let time_of_day = shared.advance_time();
        let is_night = (13_000..23_000).contains(&time_of_day);
        mobs::tick(&shared, ticks, is_night);
        drops::tick(&shared);

        // One mod tick per second keeps the JS bridge lightly loaded.
        if ticks.is_multiple_of(20) {
            shared.fire_mod(ModEvent::Tick { tick: ticks / 20 });
        }

        // Natural health regeneration every four seconds.
        if ticks.is_multiple_of(80) {
            combat::regenerate(&shared);
        }

        // Fluid flow drains its queue frequently (cheap when idle).
        if ticks.is_multiple_of(5) {
            sim::fluid_tick(&shared);
        }
        // Random-tick growth and fire every ~2 seconds.
        if ticks.is_multiple_of(40) {
            sim::random_tick(&shared);
            sim::fire_tick(&shared);
        }

        // Hunger drain every 30 seconds.
        if ticks.is_multiple_of(600) {
            combat::hunger_tick(&shared);
        }

        // Evict far-away chunks from memory every 30 seconds.
        if ticks.is_multiple_of(600) {
            evict_chunks(&shared);
        }

        // Keep-alive and world time every 10 seconds.
        if ticks.is_multiple_of(200) {
            shared.broadcast(clientbound::keep_alive(ticks as i64));
            shared.broadcast(clientbound::update_time(ticks as i64, time_of_day));
        }
    }
}

/// Drop chunks no player is near, bounding the world's memory use.
fn evict_chunks(shared: &Arc<Shared>) {
    use std::collections::HashSet;
    let r = shared.config.server.view_distance + 2;
    let mut keep: HashSet<(i32, i32)> = HashSet::new();
    for p in shared.players() {
        let (cx, cz) = p.state().chunk();
        for dz in -r..=r {
            for dx in -r..=r {
                keep.insert((cx + dx, cz + dz));
            }
        }
    }
    let removed = shared.world.lock().unwrap().retain_chunks(&keep);
    if removed > 0 {
        tracing::debug!("evicted {removed} idle chunks");
    }
}

/// Periodically persist the whole world and its players/entities.
async fn save_loop(shared: Arc<Shared>, save_dir: std::path::PathBuf, region: bool) {
    let mut interval = tokio::time::interval(Duration::from_secs(60));
    interval.tick().await; // skip the immediate first tick
    loop {
        interval.tick().await;
        save_all(&shared, &save_dir, region);
    }
}
