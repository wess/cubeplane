//! Parsing of the serverbound play packets cubeplane reacts to.
//!
//! Only the leading fields we actually use are decoded; trailing chat-signing
//! metadata (signatures, acknowledgement bitsets) is intentionally ignored
//! because cubeplane runs in unsigned/offline mode.

use cubeplane_protocol::{ProtoRead, RawPacket, Result};

use crate::ids::play_sb;

/// A decoded, actionable serverbound play packet.
#[derive(Debug, Clone)]
pub enum Play {
    TeleportConfirm { id: i32 },
    KeepAlive { id: i64 },
    ChatMessage { message: String },
    ChatCommand { command: String },
    ClientSettings { view_distance: i8 },
    SetPosition { x: f64, y: f64, z: f64, on_ground: bool },
    SetPositionRotation { x: f64, y: f64, z: f64, yaw: f32, pitch: f32, on_ground: bool },
    SetRotation { yaw: f32, pitch: f32, on_ground: bool },
    OnGround { on_ground: bool },
    HeldItem { slot: i16 },
    Animation { hand: i32 },
    /// Dig/break action. `status` 0 = start, 2 = finish (block broken).
    BlockDig { status: i32, x: i32, y: i32, z: i32, sequence: i32 },
    BlockPlace { x: i32, y: i32, z: i32, face: i32, sequence: i32 },
    /// A serverbound packet we recognise the id of but do not act on.
    Ignored,
}

/// Parse a raw play packet into a [`Play`] action.
pub fn parse_play(mut raw: RawPacket) -> Result<Play> {
    let b = &mut raw.body;
    Ok(match raw.id {
        play_sb::TELEPORT_CONFIRM => Play::TeleportConfirm { id: b.read_varint()? },
        play_sb::KEEP_ALIVE => Play::KeepAlive { id: b.read_i64()? },
        play_sb::CHAT_MESSAGE => Play::ChatMessage { message: b.read_string()? },
        play_sb::CHAT_COMMAND => Play::ChatCommand { command: b.read_string()? },
        play_sb::CLIENT_SETTINGS => {
            let _locale = b.read_string()?;
            Play::ClientSettings { view_distance: b.read_i8()? }
        }
        play_sb::POSITION => Play::SetPosition {
            x: b.read_f64()?,
            y: b.read_f64()?,
            z: b.read_f64()?,
            on_ground: b.read_bool()?,
        },
        play_sb::POSITION_LOOK => Play::SetPositionRotation {
            x: b.read_f64()?,
            y: b.read_f64()?,
            z: b.read_f64()?,
            yaw: b.read_f32()?,
            pitch: b.read_f32()?,
            on_ground: b.read_bool()?,
        },
        play_sb::LOOK => Play::SetRotation {
            yaw: b.read_f32()?,
            pitch: b.read_f32()?,
            on_ground: b.read_bool()?,
        },
        play_sb::FLYING => Play::OnGround { on_ground: b.read_bool()? },
        play_sb::HELD_ITEM_SLOT => Play::HeldItem { slot: b.read_i16()? },
        play_sb::ARM_ANIMATION => Play::Animation { hand: b.read_varint()? },
        play_sb::BLOCK_DIG => {
            let status = b.read_varint()?;
            let (x, y, z) = b.read_position()?;
            let _face = b.read_i8()?;
            let sequence = b.read_varint()?;
            Play::BlockDig { status, x, y, z, sequence }
        }
        play_sb::BLOCK_PLACE => {
            let _hand = b.read_varint()?;
            let (x, y, z) = b.read_position()?;
            let face = b.read_varint()?;
            let _cursor_x = b.read_f32()?;
            let _cursor_y = b.read_f32()?;
            let _cursor_z = b.read_f32()?;
            let _inside = b.read_bool()?;
            let sequence = b.read_varint()?;
            Play::BlockPlace { x, y, z, face, sequence }
        }
        _ => Play::Ignored,
    })
}
