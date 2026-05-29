//! Block-state registry.
//!
//! Values are global block-**state** ids for Minecraft 1.20.1 (protocol 763),
//! taken from the PrismarineJS `minecraft-data` block report (`pc/1.20`). Only
//! a curated palette is exposed; cubeplane stores raw `u16` state ids so mods
//! and generators may use any id beyond this list.

/// A global block state id (index into the flattened block-state registry).
pub type StateId = u16;

macro_rules! blocks {
    ($($name:ident = $id:expr),+ $(,)?) => {
        $(pub const $name: StateId = $id;)+

        /// Look up a curated block's state id by its `minecraft:`-style name.
        pub fn by_name(name: &str) -> Option<StateId> {
            let key = name.strip_prefix("minecraft:").unwrap_or(name);
            match key {
                $(stringify_lower!($name) => Some($name),)+
                _ => None,
            }
        }

        /// All curated `(name, id)` pairs, useful for exposing to the mod API.
        pub fn catalog() -> &'static [(&'static str, StateId)] {
            &[$((stringify_lower!($name), $name)),+]
        }
    };
}

// `stringify_lower!(GRASS_BLOCK)` => "grass_block".
macro_rules! stringify_lower {
    (AIR) => { "air" };
    (STONE) => { "stone" };
    (GRANITE) => { "granite" };
    (POLISHED_ANDESITE) => { "polished_andesite" };
    (GRASS_BLOCK) => { "grass_block" };
    (DIRT) => { "dirt" };
    (COARSE_DIRT) => { "coarse_dirt" };
    (PODZOL) => { "podzol" };
    (COBBLESTONE) => { "cobblestone" };
    (OAK_PLANKS) => { "oak_planks" };
    (BEDROCK) => { "bedrock" };
    (WATER) => { "water" };
    (SAND) => { "sand" };
    (GRAVEL) => { "gravel" };
    (OAK_LOG) => { "oak_log" };
    (OAK_LEAVES) => { "oak_leaves" };
    (GLASS) => { "glass" };
    (LAPIS_BLOCK) => { "lapis_block" };
    (CHEST) => { "chest" };
    (CRAFTING_TABLE) => { "crafting_table" };
}

blocks! {
    AIR = 0,
    STONE = 1,
    GRANITE = 2,
    POLISHED_ANDESITE = 7,
    GRASS_BLOCK = 9,
    DIRT = 10,
    COARSE_DIRT = 11,
    PODZOL = 13,
    COBBLESTONE = 14,
    OAK_PLANKS = 15,
    BEDROCK = 79,
    WATER = 80,
    SAND = 112,
    GRAVEL = 118,
    OAK_LOG = 131,
    OAK_LEAVES = 264,
    GLASS = 519,
    LAPIS_BLOCK = 522,
    CHEST = 2955,
    CRAFTING_TABLE = 4277,
}

/// Whether a state id is air (and therefore not counted toward a section's
/// non-air block count, which the client uses to skip empty sections).
#[inline]
pub fn is_air(state: StateId) -> bool {
    state == AIR
}

use crate::blocks_table::{BlockInfo, BLOCKS};

/// Look up the full block info for a state id via binary search over the
/// generated table (covers every 1.20.1 block, not just the curated set).
pub fn info(state: StateId) -> &'static BlockInfo {
    // Find the block whose [min, max] range contains `state`.
    let idx = BLOCKS
        .binary_search_by(|b| {
            if state < b.min {
                std::cmp::Ordering::Greater
            } else if state > b.max {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .unwrap_or(0); // unknown → treat as air
    &BLOCKS[idx]
}

/// Resolve any 1.20.1 block name to its default state id (full registry).
pub fn state_by_name(name: &str) -> Option<StateId> {
    let key = name.strip_prefix("minecraft:").unwrap_or(name);
    BLOCKS.iter().find(|b| b.name == key).map(|b| b.default)
}

/// How much light a block absorbs (0 = transparent, 15 = fully opaque).
#[inline]
pub fn opacity(state: StateId) -> u8 {
    info(state).opacity
}

/// Light a block emits (0..15).
#[inline]
pub fn emission(state: StateId) -> u8 {
    info(state).emission
}
