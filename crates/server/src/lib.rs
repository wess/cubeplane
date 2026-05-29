//! # cubeplane-server
//!
//! The networking and gameplay engine: it accepts TCP connections, drives the
//! handshake → status/login → play state machine, streams the world, relays
//! players to one another, bridges the JS mod runtime and exposes the control
//! API consumed by the Atlas admin panel.

mod clientbound;
mod codec;
mod config;
mod connection;
mod control;
mod ids;
mod mod_actions;
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
    let world = World::new(make_generator(&config));

    // Start the mod runtime, if enabled.
    let (mods, action_rx) = if config.mods.enabled {
        let (rt, rx) = ModRuntime::spawn(&config.mods.dir);
        info!("mods: discovered {} ({:?})", rt.loaded().len(), rt.loaded());
        (Some(rt), Some(rx))
    } else {
        (None, None)
    };

    let shared = Shared::new(config, world, mods);

    shared.fire_mod(ModEvent::ServerStart {
        version: GAME_VERSION.to_string(),
    });

    if let Some(rx) = action_rx {
        tokio::spawn(mod_actions::run(shared.clone(), rx));
    }

    tokio::spawn(game_loop(shared.clone()));

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

        // One mod tick per second keeps the JS bridge lightly loaded.
        if ticks.is_multiple_of(20) {
            shared.fire_mod(ModEvent::Tick { tick: ticks / 20 });
        }

        // Keep-alive and world time every 10 seconds.
        if ticks.is_multiple_of(200) {
            let time_of_day = (ticks as i64) % 24_000;
            shared.broadcast(clientbound::keep_alive(ticks as i64));
            shared.broadcast(clientbound::update_time(ticks as i64, time_of_day));
        }
    }
}
