//! Shared, thread-safe server state: config, world, the player table and the
//! mod runtime handle. Everything is reachable from connection tasks, the game
//! loop and the control API through an `Arc<Shared>`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use bytes::BytesMut;
use cubeplane_mods::ModRuntime;
use cubeplane_world::World;

use crate::config::Config;
use crate::entity::{ItemEntity, Mob, Projectile, Vehicle};
use crate::item::ItemStack;
use crate::player::Player;

/// Number of slots in a chest container.
pub const CONTAINER_SIZE: usize = 27;

/// Shared server state.
pub struct Shared {
    pub config: Config,
    pub world: Mutex<World>,
    players: RwLock<HashMap<i32, Player>>,
    pub(crate) mobs: RwLock<HashMap<i32, Mob>>,
    pub(crate) items: RwLock<HashMap<i32, ItemEntity>>,
    pub(crate) projectiles: RwLock<HashMap<i32, Projectile>>,
    pub(crate) vehicles: RwLock<HashMap<i32, Vehicle>>,
    /// Block-entity contents for chests, keyed by block position.
    containers: RwLock<HashMap<(i32, i32, i32), Vec<ItemStack>>>,
    /// Cells queued for fluid-flow evaluation.
    fluid_queue: Mutex<std::collections::VecDeque<(i32, i32, i32)>>,
    next_entity_id: AtomicI32,
    total_joins: AtomicU64,
    /// World time of day in ticks (0..24000), advanced by the game loop.
    world_time: std::sync::atomic::AtomicI64,
    pub mods: Option<ModRuntime>,
    /// RSA keypair for online-mode encryption (present only when enabled).
    pub server_key: Option<Arc<crate::encryption::ServerKey>>,
    started: Instant,
}

impl Shared {
    pub fn new(
        config: Config,
        world: World,
        mods: Option<ModRuntime>,
        server_key: Option<Arc<crate::encryption::ServerKey>>,
    ) -> Arc<Shared> {
        Arc::new(Shared {
            config,
            world: Mutex::new(world),
            players: RwLock::new(HashMap::new()),
            mobs: RwLock::new(HashMap::new()),
            items: RwLock::new(HashMap::new()),
            projectiles: RwLock::new(HashMap::new()),
            vehicles: RwLock::new(HashMap::new()),
            containers: RwLock::new(HashMap::new()),
            fluid_queue: Mutex::new(std::collections::VecDeque::new()),
            next_entity_id: AtomicI32::new(1),
            total_joins: AtomicU64::new(0),
            world_time: std::sync::atomic::AtomicI64::new(1000),
            mods,
            server_key,
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

    /// Register a mob in the world.
    pub fn add_mob(&self, mob: Mob) {
        self.mobs.write().unwrap().insert(mob.entity_id, mob);
    }

    /// Remove a mob by entity id.
    pub fn remove_mob(&self, entity_id: i32) -> Option<Mob> {
        self.mobs.write().unwrap().remove(&entity_id)
    }

    /// Snapshot of all live mobs.
    pub fn mobs(&self) -> Vec<Mob> {
        self.mobs.read().unwrap().values().cloned().collect()
    }

    /// Number of mobs currently in the world.
    pub fn mob_count(&self) -> usize {
        self.mobs.read().unwrap().len()
    }

    /// Mutate a mob in place under the table lock, returning the closure result.
    pub fn with_mob<R>(&self, entity_id: i32, f: impl FnOnce(&mut Mob) -> R) -> Option<R> {
        self.mobs.write().unwrap().get_mut(&entity_id).map(f)
    }

    /// Register a dropped item entity.
    pub fn add_item_entity(&self, item: ItemEntity) {
        self.items.write().unwrap().insert(item.entity_id, item);
    }

    /// Register a projectile.
    pub fn add_projectile(&self, proj: Projectile) {
        self.projectiles.write().unwrap().insert(proj.entity_id, proj);
    }

    /// Register a vehicle.
    pub fn add_vehicle(&self, v: Vehicle) {
        self.vehicles.write().unwrap().insert(v.entity_id, v);
    }

    /// Snapshot all vehicles.
    pub fn vehicles(&self) -> Vec<Vehicle> {
        self.vehicles.read().unwrap().values().cloned().collect()
    }

    /// Mutate a vehicle in place.
    pub fn with_vehicle<R>(&self, entity_id: i32, f: impl FnOnce(&mut Vehicle) -> R) -> Option<R> {
        self.vehicles.write().unwrap().get_mut(&entity_id).map(f)
    }

    /// Whether an entity id is a vehicle.
    pub fn is_vehicle(&self, entity_id: i32) -> bool {
        self.vehicles.read().unwrap().contains_key(&entity_id)
    }

    /// Queue a cell (and its 6 neighbours) for fluid-flow evaluation.
    pub fn schedule_fluid(&self, x: i32, y: i32, z: i32) {
        let mut q = self.fluid_queue.lock().unwrap();
        if q.len() > 100_000 {
            return; // safety cap
        }
        for c in [
            (x, y, z),
            (x + 1, y, z),
            (x - 1, y, z),
            (x, y + 1, z),
            (x, y - 1, z),
            (x, y, z + 1),
            (x, y, z - 1),
        ] {
            q.push_back(c);
        }
    }

    /// Drain up to `max` queued fluid cells.
    pub fn drain_fluid(&self, max: usize) -> Vec<(i32, i32, i32)> {
        let mut q = self.fluid_queue.lock().unwrap();
        let n = max.min(q.len());
        q.drain(..n).collect()
    }

    /// Create an empty container at `pos` if none exists.
    pub fn ensure_container(&self, pos: (i32, i32, i32)) {
        self.containers
            .write()
            .unwrap()
            .entry(pos)
            .or_insert_with(|| vec![ItemStack::EMPTY; CONTAINER_SIZE]);
    }

    /// Snapshot a container's contents.
    pub fn container_items(&self, pos: (i32, i32, i32)) -> Option<Vec<ItemStack>> {
        self.containers.read().unwrap().get(&pos).cloned()
    }

    /// Set one slot of a container.
    pub fn set_container_slot(&self, pos: (i32, i32, i32), idx: usize, stack: ItemStack) {
        if let Some(c) = self.containers.write().unwrap().get_mut(&pos) {
            if idx < c.len() {
                c[idx] = stack;
            }
        }
    }

    /// Remove a container (e.g. when its chest is broken), returning contents.
    pub fn remove_container(&self, pos: (i32, i32, i32)) -> Option<Vec<ItemStack>> {
        self.containers.write().unwrap().remove(&pos)
    }

    /// All containers, for persistence.
    pub fn containers_snapshot(&self) -> HashMap<(i32, i32, i32), Vec<ItemStack>> {
        self.containers.read().unwrap().clone()
    }

    /// Replace all containers (used when loading from disk).
    pub fn load_containers(&self, data: HashMap<(i32, i32, i32), Vec<ItemStack>>) {
        *self.containers.write().unwrap() = data;
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

    /// Current time of day (0..24000).
    pub fn world_time(&self) -> i64 {
        self.world_time.load(Ordering::Relaxed)
    }

    /// Advance the world clock by one tick and return the new time.
    pub fn advance_time(&self) -> i64 {
        let next = (self.world_time.load(Ordering::Relaxed) + 1) % 24_000;
        self.world_time.store(next, Ordering::Relaxed);
        next
    }

    /// Set the time of day.
    pub fn set_time(&self, time: i64) {
        self.world_time.store(time.rem_euclid(24_000), Ordering::Relaxed);
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
