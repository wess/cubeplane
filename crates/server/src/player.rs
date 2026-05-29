//! Per-player state and the handle other tasks use to reach a player.

use std::sync::Mutex;

use bytes::BytesMut;
use tokio::sync::mpsc::UnboundedSender;
use uuid::Uuid;

/// A mutable snapshot of where a player is and what they're holding.
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
}

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
    state: std::sync::Arc<Mutex<PlayerState>>,
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
        Player {
            entity_id,
            uuid,
            name,
            gamemode,
            sender,
            state: std::sync::Arc::new(Mutex::new(PlayerState::new(spawn.0, spawn.1, spawn.2))),
        }
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
}

/// Compute the deterministic offline-mode UUID for a username.
pub fn offline_uuid(name: &str) -> Uuid {
    Uuid::new_v3(&Uuid::nil(), format!("OfflinePlayer:{name}").as_bytes())
}
