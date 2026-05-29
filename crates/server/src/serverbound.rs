//! Parsing of the serverbound play packets cubeplane reacts to.
//!
//! Only the leading fields we actually use are decoded; trailing chat-signing
//! metadata (signatures, acknowledgement bitsets) is intentionally ignored
//! because cubeplane runs in unsigned/offline mode.

use bytes::Buf;

use cubeplane_protocol::{ProtoRead, RawPacket, Result};

use crate::ids::play_sb;
use crate::item::ItemStack;

/// Read an item stack in the `slot` wire format, consuming any trailing NBT.
fn read_slot<B: Buf>(b: &mut B) -> Result<ItemStack> {
    if !b.read_bool()? {
        return Ok(ItemStack::EMPTY);
    }
    let id = b.read_varint()?;
    let count = b.read_i8()? as u8;
    // optionalNbt: a lone 0x00 (TAG_End) means none; otherwise a named
    // compound. `Nbt::from_bytes` consumes exactly the right bytes in both
    // cases (it reads the type byte and, for non-compound, stops).
    let _ = cubeplane_nbt::Nbt::from_bytes(b);
    Ok(ItemStack::new(id, count))
}

/// A decoded, actionable serverbound play packet. Some fields are parsed for
/// completeness/documentation even where the engine does not yet act on them.
#[derive(Debug, Clone)]
#[allow(dead_code)]
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
    /// Client status command. `action` 0 = perform respawn, 1 = request stats.
    ClientCommand { action: i32 },
    /// Interact with an entity. `interaction` 0 = interact, 1 = attack.
    UseEntity { target: i32, interaction: i32 },
    /// Right-click / use the held item (e.g. eating food).
    UseItem,
    /// Creative-mode direct slot edit.
    CreativeSlot { slot: i16, stack: ItemStack },
    /// A click in an inventory window. We trust the client-reported result
    /// slots, which keeps the model simple while supporting all click modes.
    WindowClick { changed: Vec<(i16, ItemStack)>, cursor: ItemStack },
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
        play_sb::CLIENT_COMMAND => Play::ClientCommand { action: b.read_varint()? },
        play_sb::USE_ITEM => Play::UseItem,
        play_sb::SET_CREATIVE_SLOT => {
            let slot = b.read_i16()?;
            let stack = read_slot(b)?;
            Play::CreativeSlot { slot, stack }
        }
        play_sb::WINDOW_CLICK => {
            let _window = b.read_u8()?;
            let _state = b.read_varint()?;
            let _slot = b.read_i16()?;
            let _button = b.read_i8()?;
            let _mode = b.read_varint()?;
            let n = b.read_varint()? as usize;
            let mut changed = Vec::with_capacity(n.min(64));
            for _ in 0..n {
                let location = b.read_i16()?;
                let stack = read_slot(b)?;
                changed.push((location, stack));
            }
            let cursor = read_slot(b)?;
            Play::WindowClick { changed, cursor }
        }
        play_sb::USE_ENTITY => {
            let target = b.read_varint()?;
            let interaction = b.read_varint()?;
            // Remaining fields (cursor/hand/sneaking) are not needed here.
            Play::UseEntity { target, interaction }
        }
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
