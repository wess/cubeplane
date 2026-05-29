//! Packet id constants for Minecraft Java protocol **763** (1.20.1).
//!
//! Values were extracted from the PrismarineJS `minecraft-data` protocol
//! definition for `pc/1.20`. Ids are state- and direction-relative.
//!
//! This is a reference table; not every id is wired up yet, so unused
//! constants are expected.
#![allow(dead_code)]

/// Handshaking, serverbound.
pub mod handshake_sb {
    pub const SET_PROTOCOL: i32 = 0x00;
}

/// Status, clientbound.
pub mod status_cb {
    pub const SERVER_INFO: i32 = 0x00;
    pub const PONG: i32 = 0x01;
}

/// Status, serverbound.
pub mod status_sb {
    pub const REQUEST: i32 = 0x00;
    pub const PING: i32 = 0x01;
}

/// Login, clientbound.
pub mod login_cb {
    pub const DISCONNECT: i32 = 0x00;
    pub const ENCRYPTION_REQUEST: i32 = 0x01;
    pub const SUCCESS: i32 = 0x02;
    pub const SET_COMPRESSION: i32 = 0x03;
}

/// Login, serverbound.
pub mod login_sb {
    pub const LOGIN_START: i32 = 0x00;
    pub const ENCRYPTION_RESPONSE: i32 = 0x01;
    /// Login Acknowledged (1.20.2+): the client confirms login and enters the
    /// Configuration state.
    pub const LOGIN_ACKNOWLEDGED: i32 = 0x03;
}

/// Configuration state (1.20.2+), clientbound.
pub mod config_cb {
    pub const FINISH_CONFIGURATION: i32 = 0x02;
    pub const REGISTRY_DATA: i32 = 0x05;
}

/// Configuration state (1.20.2+), serverbound.
pub mod config_sb {
    pub const CLIENT_INFORMATION: i32 = 0x00;
    pub const PLUGIN_MESSAGE: i32 = 0x01;
    pub const FINISH_CONFIGURATION: i32 = 0x02;
    pub const KEEP_ALIVE: i32 = 0x03;
    pub const PONG: i32 = 0x04;
    pub const RESOURCE_PACK: i32 = 0x05;
}

/// Play, clientbound.
pub mod play_cb {
    pub const SPAWN_ENTITY: i32 = 0x01;
    pub const SPAWN_XP_ORB: i32 = 0x02;
    pub const SPAWN_PLAYER: i32 = 0x03;
    pub const AWARD_STATISTICS: i32 = 0x05;
    pub const TRADE_LIST: i32 = 0x2a;
    pub const SET_PASSENGERS: i32 = 0x59;
    pub const DECLARE_COMMANDS: i32 = 0x10;
    pub const CLOSE_WINDOW: i32 = 0x11;
    pub const WINDOW_ITEMS: i32 = 0x12;
    pub const WINDOW_PROPERTY: i32 = 0x15;
    pub const SET_SLOT: i32 = 0x14;
    pub const EXPLOSION: i32 = 0x1d;
    pub const WORLD_PARTICLES: i32 = 0x26;
    pub const OPEN_WINDOW: i32 = 0x30;
    pub const OPEN_SIGN_EDITOR: i32 = 0x31;
    pub const SET_ACTION_BAR: i32 = 0x46;
    pub const ENTITY_METADATA: i32 = 0x52;
    pub const SET_EXPERIENCE: i32 = 0x56;
    pub const SET_TITLE_SUBTITLE: i32 = 0x5d;
    pub const SET_TITLE_TEXT: i32 = 0x5f;
    pub const SET_TITLE_TIME: i32 = 0x60;
    pub const SOUND_EFFECT: i32 = 0x62;
    pub const TAB_LIST_HEADER: i32 = 0x65;
    pub const COLLECT: i32 = 0x67;
    /// Update Advancements — follows ENTITY_TELEPORT (0x68) in the 1.20.1 order.
    pub const UPDATE_ADVANCEMENTS: i32 = 0x69;
    pub const ENTITY_EFFECT: i32 = 0x6c;
    pub const ACKNOWLEDGE_BLOCK_CHANGE: i32 = 0x06;
    pub const BLOCK_UPDATE: i32 = 0x0a;
    pub const DAMAGE_EVENT: i32 = 0x18;
    pub const ENTITY_STATUS: i32 = 0x1c;
    pub const DISCONNECT: i32 = 0x1a;
    pub const UNLOAD_CHUNK: i32 = 0x1e;
    pub const GAME_EVENT: i32 = 0x1f;
    pub const HURT_ANIMATION: i32 = 0x21;
    pub const INITIALIZE_WORLD_BORDER: i32 = 0x22;
    pub const KEEP_ALIVE: i32 = 0x23;
    pub const CHUNK_DATA: i32 = 0x24;
    pub const LOGIN: i32 = 0x28;
    pub const ENTITY_POSITION: i32 = 0x2b;
    pub const ENTITY_POSITION_ROTATION: i32 = 0x2c;
    pub const ENTITY_ROTATION: i32 = 0x2d;
    pub const PLAYER_ABILITIES: i32 = 0x34;
    pub const PLAYER_CHAT: i32 = 0x35;
    pub const DEATH_COMBAT_EVENT: i32 = 0x38;
    pub const PLAYER_INFO_REMOVE: i32 = 0x39;
    pub const PLAYER_INFO_UPDATE: i32 = 0x3a;
    pub const SYNC_POSITION: i32 = 0x3c;
    pub const REMOVE_ENTITIES: i32 = 0x3e;
    pub const REMOVE_ENTITY_EFFECT: i32 = 0x3f;
    pub const RESOURCE_PACK: i32 = 0x40;
    pub const RESPAWN: i32 = 0x41;
    pub const ENTITY_HEAD_ROTATION: i32 = 0x42;
    pub const SET_HELD_ITEM: i32 = 0x4d;
    pub const SET_CENTER_CHUNK: i32 = 0x4e;
    pub const SPAWN_POSITION: i32 = 0x50;
    pub const ENTITY_VELOCITY: i32 = 0x54;
    pub const UPDATE_HEALTH: i32 = 0x57;
    pub const UPDATE_TIME: i32 = 0x5e;
    pub const SYSTEM_CHAT: i32 = 0x64;
    pub const ENTITY_TELEPORT: i32 = 0x68;
}

/// Play, serverbound.
pub mod play_sb {
    pub const TELEPORT_CONFIRM: i32 = 0x00;
    pub const CHAT_COMMAND: i32 = 0x04;
    pub const CHAT_MESSAGE: i32 = 0x05;
    pub const CLIENT_COMMAND: i32 = 0x07;
    pub const CLIENT_SETTINGS: i32 = 0x08;
    pub const KEEP_ALIVE: i32 = 0x12;
    pub const POSITION: i32 = 0x14;
    pub const POSITION_LOOK: i32 = 0x15;
    pub const LOOK: i32 = 0x16;
    pub const FLYING: i32 = 0x17;
    pub const HELD_ITEM_SLOT: i32 = 0x28;
    pub const USE_ENTITY: i32 = 0x10;
    pub const VEHICLE_MOVE: i32 = 0x18;
    pub const STEER_VEHICLE: i32 = 0x1f;
    pub const SELECT_TRADE: i32 = 0x26;
    pub const UPDATE_SIGN: i32 = 0x2e;
    pub const WINDOW_CLICK: i32 = 0x0b;
    pub const CLOSE_WINDOW: i32 = 0x0c;
    pub const SET_CREATIVE_SLOT: i32 = 0x2b;
    pub const BLOCK_DIG: i32 = 0x1d;
    pub const ARM_ANIMATION: i32 = 0x2f;
    pub const BLOCK_PLACE: i32 = 0x31;
    pub const USE_ITEM: i32 = 0x32;
}
