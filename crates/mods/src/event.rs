//! Events the engine fires into the mod runtime, and actions it gets back.

use serde::Deserialize;
use serde_json::{json, Value};

/// An event dispatched to JS mod handlers. Each variant maps to an event name
/// and a JSON payload object passed to `cubeplane.on(name, ...)` handlers.
#[derive(Debug, Clone)]
pub enum ModEvent {
    ServerStart { version: String },
    ServerStop,
    Tick { tick: u64 },
    PlayerJoin { player: String, uuid: String, entity_id: i32 },
    PlayerLeave { player: String },
    Chat { player: String, message: String },
    Command { player: String, command: String, args: Vec<String> },
    BlockPlace { player: String, x: i32, y: i32, z: i32, block: String },
    BlockBreak { player: String, x: i32, y: i32, z: i32 },
}

impl ModEvent {
    /// The JS event name and its payload object.
    pub fn to_js(&self) -> (&'static str, Value) {
        match self {
            ModEvent::ServerStart { version } => ("server_start", json!({ "version": version })),
            ModEvent::ServerStop => ("server_stop", json!({})),
            ModEvent::Tick { tick } => ("tick", json!({ "tick": tick })),
            ModEvent::PlayerJoin { player, uuid, entity_id } => (
                "player_join",
                json!({ "player": player, "uuid": uuid, "entityId": entity_id }),
            ),
            ModEvent::PlayerLeave { player } => ("player_leave", json!({ "player": player })),
            ModEvent::Chat { player, message } => {
                ("chat", json!({ "player": player, "message": message }))
            }
            ModEvent::Command { player, command, args } => (
                "command",
                json!({ "player": player, "command": command, "args": args }),
            ),
            ModEvent::BlockPlace { player, x, y, z, block } => (
                "block_place",
                json!({ "player": player, "x": x, "y": y, "z": z, "block": block }),
            ),
            ModEvent::BlockBreak { player, x, y, z } => (
                "block_break",
                json!({ "player": player, "x": x, "y": y, "z": z }),
            ),
        }
    }

    /// Whether this event should be routed through the command dispatcher.
    pub fn is_command(&self) -> bool {
        matches!(self, ModEvent::Command { .. })
    }
}

/// An effect a mod requested, drained from JS after dispatch and executed by
/// the engine.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum ModAction {
    #[serde(rename = "broadcast")]
    Broadcast { message: String },
    #[serde(rename = "tell")]
    Tell { player: String, message: String },
    #[serde(rename = "log")]
    Log { message: String },
    #[serde(rename = "set_block")]
    SetBlock { x: i32, y: i32, z: i32, block: String },
    #[serde(rename = "kick")]
    Kick { player: String, reason: String },
    /// Forward-compatible catch-all for action types this build doesn't know.
    #[serde(other)]
    Unknown,
}
