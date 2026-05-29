//! # cubeplane-server
//!
//! The networking and gameplay engine: it accepts TCP connections, drives the
//! handshake → status/login → play state machine, streams the world, relays
//! players to one another, bridges the JS mod runtime and exposes the control
//! API consumed by the Atlas admin panel.

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
mod mod_actions;
mod persistence;
mod player;
mod registry;
mod serverbound;
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

    // Load persisted block edits.
    let save_dir = std::path::PathBuf::from(&config.world.save_dir);
    if config.world.save {
        let edits = persistence::load_blocks(&save_dir);
        if !edits.is_empty() {
            info!("loaded {} saved block edit(s)", edits.len());
        }
        world.load_edits(edits);
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
    }

    shared.fire_mod(ModEvent::ServerStart {
        version: GAME_VERSION.to_string(),
    });

    if let Some(rx) = action_rx {
        tokio::spawn(mod_actions::run(shared.clone(), rx));
    }

    tokio::spawn(game_loop(shared.clone()));

    if shared.config.world.save {
        tokio::spawn(save_loop(shared.clone(), save_dir));
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

/// Periodically persist world edits and online players' data.
async fn save_loop(shared: Arc<Shared>, save_dir: std::path::PathBuf) {
    let mut interval = tokio::time::interval(Duration::from_secs(60));
    interval.tick().await; // skip the immediate first tick
    loop {
        interval.tick().await;
        let edits = shared.world.lock().unwrap().edits().clone();
        if let Err(e) = persistence::save_blocks(&save_dir, &edits) {
            error!("failed to save world: {e}");
        }

        // Persist chest contents.
        let entries: Vec<_> = shared
            .containers_snapshot()
            .into_iter()
            .map(|(pos, stacks)| {
                let items: Vec<(i32, u8)> = stacks.iter().map(|s| (s.id, s.count)).collect();
                (pos, items)
            })
            .collect();
        let _ = persistence::save_containers(&save_dir, &entries);

        for player in shared.players() {
            let _ = persistence::save_player(&save_dir, player.uuid, &player.snapshot_data());
        }
    }
}
