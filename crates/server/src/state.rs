//! Shared, thread-safe server state: config, world, the player table and the
//! mod runtime handle. Everything is reachable from connection tasks, the game
//! loop and the control API through an `Arc<Shared>`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use bytes::BytesMut;
use cubeplane_mods::ModRuntime;
use cubeplane_world::{EndGenerator, NetherGenerator, World};

use crate::ai::Turn;
use crate::config::{AiConfig, Config};
use crate::entity::{ItemEntity, Mob, Projectile, Vehicle};
use crate::item::ItemStack;
use crate::player::Player;

/// Number of slots in a chest container.
pub const CONTAINER_SIZE: usize = 27;

/// A furnace block entity: input/fuel/output plus cook & burn timers.
#[derive(Clone, Default)]
pub struct Furnace {
    pub input: ItemStack,
    pub fuel: ItemStack,
    pub output: ItemStack,
    /// Cook progress in ticks (0..200).
    pub cook: u32,
    /// Remaining burn time of the current fuel unit.
    pub burn: u32,
    /// Burn time the current fuel unit started with.
    pub burn_total: u32,
}

/// The personality and running conversation of an AI villager.
pub struct VillagerBrain {
    pub profession: &'static str,
    pub name: String,
    pub history: Vec<Turn>,
    /// True while a request to the model is in flight (single-flight).
    pub busy: bool,
}

/// Shared server state.
pub struct Shared {
    pub config: Config,
    pub world: Mutex<World>,
    /// The Nether and End dimensions (overworld is `world`).
    nether: Mutex<World>,
    the_end: Mutex<World>,
    players: RwLock<HashMap<i32, Player>>,
    pub(crate) mobs: RwLock<HashMap<i32, Mob>>,
    pub(crate) items: RwLock<HashMap<i32, ItemEntity>>,
    pub(crate) projectiles: RwLock<HashMap<i32, Projectile>>,
    pub(crate) vehicles: RwLock<HashMap<i32, Vehicle>>,
    /// Block-entity contents for chests, keyed by block position.
    containers: RwLock<HashMap<(i32, i32, i32), Vec<ItemStack>>>,
    /// Sign text (4 lines) keyed by block position.
    signs: RwLock<HashMap<(i32, i32, i32), [String; 4]>>,
    /// Furnace block entities keyed by block position.
    pub(crate) furnaces: RwLock<HashMap<(i32, i32, i32), Furnace>>,
    /// Cells queued for fluid-flow evaluation.
    fluid_queue: Mutex<std::collections::VecDeque<(i32, i32, i32)>>,
    next_entity_id: AtomicI32,
    total_joins: AtomicU64,
    /// World time of day in ticks (0..24000), advanced by the game loop.
    world_time: std::sync::atomic::AtomicI64,
    /// Whether it is currently raining.
    raining: std::sync::atomic::AtomicBool,
    pub mods: Option<ModRuntime>,
    /// RSA keypair for online-mode encryption (present only when enabled).
    pub server_key: Option<Arc<crate::encryption::ServerKey>>,
    /// Live, runtime-editable AI villager configuration.
    ai: RwLock<AiConfig>,
    /// Per-villager personalities and conversations.
    villagers: RwLock<HashMap<i32, VillagerBrain>>,
    started: Instant,
}

impl Shared {
    pub fn new(
        config: Config,
        world: World,
        mods: Option<ModRuntime>,
        server_key: Option<Arc<crate::encryption::ServerKey>>,
    ) -> Arc<Shared> {
        let config_ai = config.ai.clone();
        Arc::new(Shared {
            config,
            world: Mutex::new(world),
            nether: Mutex::new(World::new(Arc::new(NetherGenerator))),
            the_end: Mutex::new(World::new(Arc::new(EndGenerator))),
            players: RwLock::new(HashMap::new()),
            mobs: RwLock::new(HashMap::new()),
            items: RwLock::new(HashMap::new()),
            projectiles: RwLock::new(HashMap::new()),
            vehicles: RwLock::new(HashMap::new()),
            containers: RwLock::new(HashMap::new()),
            signs: RwLock::new(HashMap::new()),
            furnaces: RwLock::new(HashMap::new()),
            fluid_queue: Mutex::new(std::collections::VecDeque::new()),
            next_entity_id: AtomicI32::new(1),
            total_joins: AtomicU64::new(0),
            world_time: std::sync::atomic::AtomicI64::new(1000),
            raining: std::sync::atomic::AtomicBool::new(false),
            ai: RwLock::new(config_ai),
            villagers: RwLock::new(HashMap::new()),
            mods,
            server_key,
            started: Instant::now(),
        })
    }

    /// The world for a dimension number (0 overworld, 1 nether, 2 end).
    pub fn dim_world(&self, dim: u8) -> &Mutex<World> {
        match dim {
            1 => &self.nether,
            2 => &self.the_end,
            _ => &self.world,
        }
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

    /// Snapshot all dropped item entities.
    pub fn item_entities(&self) -> Vec<ItemEntity> {
        self.items.read().unwrap().values().cloned().collect()
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

    /// Ensure a furnace block entity exists at `pos`.
    pub fn ensure_furnace(&self, pos: (i32, i32, i32)) {
        self.furnaces.write().unwrap().entry(pos).or_default();
    }

    /// Mutate a furnace block entity.
    pub fn with_furnace<R>(&self, pos: (i32, i32, i32), f: impl FnOnce(&mut Furnace) -> R) -> Option<R> {
        self.furnaces.write().unwrap().get_mut(&pos).map(f)
    }

    /// Remove a furnace, returning its contents.
    pub fn remove_furnace(&self, pos: (i32, i32, i32)) -> Option<Furnace> {
        self.furnaces.write().unwrap().remove(&pos)
    }

    /// Positions of all furnaces (for the smelting tick).
    pub fn furnace_positions(&self) -> Vec<(i32, i32, i32)> {
        self.furnaces.read().unwrap().keys().copied().collect()
    }

    /// Store the text on a sign.
    pub fn set_sign(&self, pos: (i32, i32, i32), lines: [String; 4]) {
        self.signs.write().unwrap().insert(pos, lines);
    }

    /// Read a sign's text.
    pub fn sign(&self, pos: (i32, i32, i32)) -> Option<[String; 4]> {
        self.signs.read().unwrap().get(&pos).cloned()
    }

    /// Remove a sign's text (when broken).
    pub fn remove_sign(&self, pos: (i32, i32, i32)) {
        self.signs.write().unwrap().remove(&pos);
    }

    /// All signs, for persistence.
    pub fn signs_snapshot(&self) -> HashMap<(i32, i32, i32), [String; 4]> {
        self.signs.read().unwrap().clone()
    }

    /// Replace all signs (loading from disk).
    pub fn load_signs(&self, data: HashMap<(i32, i32, i32), [String; 4]>) {
        *self.signs.write().unwrap() = data;
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

    /// Whether it is raining.
    pub fn raining(&self) -> bool {
        self.raining.load(Ordering::Relaxed)
    }

    /// Set the weather state.
    pub fn set_raining(&self, raining: bool) {
        self.raining.store(raining, Ordering::Relaxed);
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

    /// Current AI configuration snapshot.
    pub fn ai_config(&self) -> AiConfig {
        self.ai.read().unwrap().clone()
    }

    /// Replace the live AI configuration (e.g. from the admin panel).
    pub fn set_ai_config(&self, cfg: AiConfig) {
        *self.ai.write().unwrap() = cfg;
    }

    /// Register a villager's personality if not already known.
    pub fn register_villager(&self, entity_id: i32) {
        let mut v = self.villagers.write().unwrap();
        v.entry(entity_id).or_insert_with(|| VillagerBrain {
            profession: crate::ai::profession_for(entity_id),
            name: crate::ai::name_for(entity_id),
            history: Vec::new(),
            busy: false,
        });
    }

    /// Forget a villager (when it dies/despawns).
    pub fn remove_villager(&self, entity_id: i32) {
        self.villagers.write().unwrap().remove(&entity_id);
    }

    /// A villager's display name and profession, if registered.
    pub fn villager_identity(&self, entity_id: i32) -> Option<(String, &'static str)> {
        self.villagers
            .read()
            .unwrap()
            .get(&entity_id)
            .map(|b| (b.name.clone(), b.profession))
    }

    /// Mutate a villager's brain.
    pub fn with_villager<R>(&self, entity_id: i32, f: impl FnOnce(&mut VillagerBrain) -> R) -> Option<R> {
        self.villagers.write().unwrap().get_mut(&entity_id).map(f)
    }

    /// Fire an event into the mod runtime, if mods are enabled.
    pub fn fire_mod(&self, event: cubeplane_mods::ModEvent) {
        if let Some(m) = &self.mods {
            m.fire(event);
        }
    }
}
