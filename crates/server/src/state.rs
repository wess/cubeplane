//! Shared, thread-safe server state: config, world, the player table and the
//! mod runtime handle. Everything is reachable from connection tasks, the game
//! loop and the control API through an `Arc<Shared>`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use bytes::BytesMut;
use cubeplane_mods::ModRuntime;
use cubeplane_world::{block, World};

use crate::config::Config;
use crate::player::Player;

/// The nine hotbar blocks players build with, slot 0..9.
pub const HOTBAR: [(&str, u16); 9] = [
    ("stone", block::STONE),
    ("dirt", block::DIRT),
    ("cobblestone", block::COBBLESTONE),
    ("oak_planks", block::OAK_PLANKS),
    ("glass", block::GLASS),
    ("sand", block::SAND),
    ("oak_log", block::OAK_LOG),
    ("oak_leaves", block::OAK_LEAVES),
    ("grass_block", block::GRASS_BLOCK),
];

/// Shared server state.
pub struct Shared {
    pub config: Config,
    pub world: Mutex<World>,
    players: RwLock<HashMap<i32, Player>>,
    next_entity_id: AtomicI32,
    total_joins: AtomicU64,
    pub mods: Option<ModRuntime>,
    started: Instant,
}

impl Shared {
    pub fn new(config: Config, world: World, mods: Option<ModRuntime>) -> Arc<Shared> {
        Arc::new(Shared {
            config,
            world: Mutex::new(world),
            players: RwLock::new(HashMap::new()),
            next_entity_id: AtomicI32::new(1),
            total_joins: AtomicU64::new(0),
            mods,
            started: Instant::now(),
        })
    }

    /// Allocate a fresh, unique entity id.
    pub fn next_entity_id(&self) -> i32 {
        self.next_entity_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Register a player in the table.
    pub fn add_player(&self, player: Player) {
        self.total_joins.fetch_add(1, Ordering::Relaxed);
        self.players.write().unwrap().insert(player.entity_id, player);
    }

    /// Remove a player, returning the removed handle if present.
    pub fn remove_player(&self, entity_id: i32) -> Option<Player> {
        self.players.write().unwrap().remove(&entity_id)
    }

    /// Snapshot of all connected players.
    pub fn players(&self) -> Vec<Player> {
        self.players.read().unwrap().values().cloned().collect()
    }

    /// Number of connected players.
    pub fn player_count(&self) -> usize {
        self.players.read().unwrap().len()
    }

    /// Look up a player by (case-insensitive) name.
    pub fn player_by_name(&self, name: &str) -> Option<Player> {
        self.players
            .read()
            .unwrap()
            .values()
            .find(|p| p.name.eq_ignore_ascii_case(name))
            .cloned()
    }

    /// Seconds the server has been running.
    pub fn uptime_secs(&self) -> u64 {
        self.started.elapsed().as_secs()
    }

    /// Total joins observed since startup.
    pub fn total_joins(&self) -> u64 {
        self.total_joins.load(Ordering::Relaxed)
    }

    /// Send a payload to every connected player.
    pub fn broadcast(&self, payload: BytesMut) {
        for p in self.players.read().unwrap().values() {
            p.send(payload.clone());
        }
    }

    /// Send a payload to everyone except one entity.
    pub fn broadcast_except(&self, except: i32, payload: BytesMut) {
        for p in self.players.read().unwrap().values() {
            if p.entity_id != except {
                p.send(payload.clone());
            }
        }
    }

    /// Fire an event into the mod runtime, if mods are enabled.
    pub fn fire_mod(&self, event: cubeplane_mods::ModEvent) {
        if let Some(m) = &self.mods {
            m.fire(event);
        }
    }
}

/// Resolve a hotbar slot to its `(name, state id)` block, clamped into range.
pub fn hotbar_block(slot: u8) -> (&'static str, u16) {
    HOTBAR[(slot as usize).min(HOTBAR.len() - 1)]
}
