//! Per-player state and the handle other tasks use to reach a player.

use std::sync::{Arc, Mutex};

use bytes::BytesMut;
use tokio::sync::mpsc::UnboundedSender;
use uuid::Uuid;

use crate::inventory::Inventory;

/// A mutable snapshot of where a player is, their vitals and what they hold.
#[derive(Debug, Clone, Copy)]
pub struct PlayerState {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
    /// Selected hotbar slot (0..9).
    pub held_slot: u8,
    /// Health in half-hearts (0..=20).
    pub health: f32,
    /// Food level (0..=20).
    pub food: i32,
    /// Food saturation.
    pub saturation: f32,
    /// True once health hit zero, until the player respawns.
    pub dead: bool,
    /// Highest Y reached since last touching the ground, for fall damage.
    pub fall_peak_y: f64,
    /// Total accumulated experience points.
    pub xp_total: i32,
    /// Ticks remaining before the next contact-damage tick can apply.
    pub hurt_cooldown: u32,
    /// Current gamemode (0 survival, 1 creative, 2 adventure, 3 spectator).
    pub gamemode: i32,
    /// Block position of the chest the player currently has open, if any.
    pub open_container: Option<(i32, i32, i32)>,
    /// True while a (non-inventory) merchant window is open.
    pub open_merchant: bool,
    /// Entity id of the vehicle the player is riding, if any.
    pub riding: Option<i32>,
}

/// Maximum player health, in half-hearts.
pub const MAX_HEALTH: f32 = 20.0;

impl PlayerState {
    pub fn new(x: f64, y: f64, z: f64) -> Self {
        PlayerState {
            x,
            y,
            z,
            yaw: 0.0,
            pitch: 0.0,
            on_ground: true,
            held_slot: 0,
            health: MAX_HEALTH,
            food: 20,
            saturation: 5.0,
            dead: false,
            fall_peak_y: y,
            xp_total: 0,
            hurt_cooldown: 0,
            gamemode: 0,
            open_container: None,
            open_merchant: false,
            riding: None,
        }
    }

    /// Chunk coordinates the player currently occupies.
    pub fn chunk(&self) -> (i32, i32) {
        (
            (self.x.floor() as i32).div_euclid(16),
            (self.z.floor() as i32).div_euclid(16),
        )
    }
}

/// A connected player. Cloneable handle stored in the server's player table;
/// the `sender` pushes encoded packet payloads to that player's writer task.
#[derive(Clone)]
pub struct Player {
    pub entity_id: i32,
    pub uuid: Uuid,
    pub name: String,
    pub gamemode: i32,
    pub sender: UnboundedSender<BytesMut>,
    state: Arc<Mutex<PlayerState>>,
    inventory: Arc<Mutex<Inventory>>,
}

impl Player {
    pub fn new(
        entity_id: i32,
        uuid: Uuid,
        name: String,
        gamemode: i32,
        sender: UnboundedSender<BytesMut>,
        spawn: (f64, f64, f64),
    ) -> Self {
        let mut st = PlayerState::new(spawn.0, spawn.1, spawn.2);
        st.gamemode = gamemode;
        Player {
            entity_id,
            uuid,
            name,
            gamemode,
            sender,
            state: Arc::new(Mutex::new(st)),
            inventory: Arc::new(Mutex::new(Inventory::default())),
        }
    }

    /// The player's current gamemode (authoritative; may change at runtime).
    pub fn gamemode(&self) -> i32 {
        self.state().gamemode
    }

    /// Mutate the inventory under its lock.
    pub fn inventory<R>(&self, f: impl FnOnce(&mut Inventory) -> R) -> R {
        let mut guard = self.inventory.lock().unwrap();
        f(&mut guard)
    }

    /// Read the current player state.
    pub fn state(&self) -> PlayerState {
        *self.state.lock().unwrap()
    }

    /// Mutate the player state under its lock.
    pub fn update<R>(&self, f: impl FnOnce(&mut PlayerState) -> R) -> R {
        let mut guard = self.state.lock().unwrap();
        f(&mut guard)
    }

    /// Queue a packet payload (id + body) to be sent to this player.
    pub fn send(&self, payload: BytesMut) {
        let _ = self.sender.send(payload);
    }

    /// Whether the player is currently dead (awaiting respawn).
    pub fn is_dead(&self) -> bool {
        self.state().dead
    }

    /// Send the player their full inventory (Set Container Content).
    pub fn sync_inventory(&self) {
        let stacks = self.inventory(|inv| inv.slots().to_vec());
        self.send(crate::clientbound::window_items(0, 0, &stacks, crate::item::ItemStack::EMPTY));
    }

    /// Set one inventory slot and push the update to the client.
    pub fn set_slot(&self, slot: usize, stack: crate::item::ItemStack) {
        self.inventory(|inv| inv.set(slot, stack));
        self.send(crate::clientbound::set_slot(0, 0, slot as i16, stack));
    }

    /// Give the player items, syncing the result.
    pub fn give(&self, id: i32, count: u8) {
        self.inventory(|inv| inv.add(id, count));
        self.sync_inventory();
    }

    /// Capture this player's persistable state.
    pub fn snapshot_data(&self) -> crate::persistence::PlayerData {
        let s = self.state();
        let items = self.inventory(|inv| {
            inv.slots()
                .iter()
                .enumerate()
                .filter(|(_, st)| !st.is_empty())
                .map(|(i, st)| (i as u16, st.id, st.count))
                .collect()
        });
        crate::persistence::PlayerData {
            x: s.x,
            y: s.y,
            z: s.z,
            yaw: s.yaw,
            pitch: s.pitch,
            health: s.health,
            food: s.food,
            saturation: s.saturation,
            xp_total: s.xp_total,
            items,
        }
    }

    /// Restore saved state into this player (before the join packets are sent).
    pub fn apply_data(&self, d: &crate::persistence::PlayerData) {
        self.update(|s| {
            s.x = d.x;
            s.y = d.y;
            s.z = d.z;
            s.yaw = d.yaw;
            s.pitch = d.pitch;
            s.health = d.health;
            s.food = d.food;
            s.saturation = d.saturation;
            s.fall_peak_y = d.y;
            s.xp_total = d.xp_total;
        });
        self.inventory(|inv| {
            for (slot, id, count) in &d.items {
                inv.set(*slot as usize, crate::item::ItemStack::new(*id, *count));
            }
        });
    }
}

/// Compute the deterministic offline-mode UUID for a username.
pub fn offline_uuid(name: &str) -> Uuid {
    Uuid::new_v3(&Uuid::nil(), format!("OfflinePlayer:{name}").as_bytes())
}
