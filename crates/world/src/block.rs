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

/// Orient a block's default state for placement, using the clicked `face`
/// (0=-Y,1=+Y,2=-Z,3=+Z,4=-X,5=+X) and the player `yaw` (degrees). Handles the
/// common `axis`, horizontal/all `facing`, and stair `half`/slab `type`
/// properties generically from the block-state property table; other
/// properties keep their default value.
pub fn place_state(default: StateId, face: i32, yaw: f32) -> StateId {
    let bi = info(default);
    if bi.props.is_empty() {
        return default;
    }
    let n = bi.props.len();
    // Strides: the last property varies fastest.
    let mut stride = vec![1u32; n];
    for i in (0..n.saturating_sub(1)).rev() {
        stride[i] = stride[i + 1] * bi.props[i + 1].values as u32;
    }
    let offset = (default - bi.min) as u32;
    let mut idx: Vec<u32> = (0..n)
        .map(|i| (offset / stride[i]) % bi.props[i].values as u32)
        .collect();

    for (slot, p) in idx.iter_mut().zip(bi.props.iter()) {
        match (p.name, p.values) {
            ("axis", 3) => *slot = axis_for_face(face),
            ("facing", 4) => *slot = facing4_from_yaw(yaw),
            ("facing", 6) => *slot = facing6_for_face(face),
            ("half", 2) => *slot = 1, // bottom
            ("type", 3) => *slot = 1, // bottom slab
            _ => {}
        }
    }

    let s = bi.min as u32 + idx.iter().zip(stride.iter()).map(|(i, st)| i * st).sum::<u32>();
    s as StateId
}

/// axis enum order is `[x, y, z]`.
fn axis_for_face(face: i32) -> u32 {
    match face {
        4 | 5 => 0, // ±X
        2 | 3 => 2, // ±Z
        _ => 1,     // ±Y
    }
}

/// 4-value facing order `[north, south, west, east]`; a block faces the way the
/// player looks.
fn facing4_from_yaw(yaw: f32) -> u32 {
    let y = yaw.rem_euclid(360.0);
    if (45.0..135.0).contains(&y) {
        2 // west
    } else if (135.0..225.0).contains(&y) {
        0 // north
    } else if (225.0..315.0).contains(&y) {
        3 // east
    } else {
        1 // south
    }
}

/// 6-value facing order `[down, up, north, south, west, east]`.
fn facing6_for_face(face: i32) -> u32 {
    match face {
        0 => 0,
        1 => 1,
        2 => 2,
        3 => 3,
        4 => 4,
        _ => 5,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn place_state_orients_logs_and_stairs() {
        // oak_log default 131 (axis=y). Side faces give x/z; top/bottom give y.
        assert_eq!(place_state(131, 4, 0.0), 130); // ±X → axis x
        assert_eq!(place_state(131, 1, 0.0), 131); // +Y → axis y
        assert_eq!(place_state(131, 2, 0.0), 132); // ±Z → axis z
        // oak_stairs facing reflects yaw; result stays within the block's range.
        let s = place_state(2885, 1, 270.0);
        assert!((2874..=2953).contains(&s));
    }

    #[test]
    fn full_registry_resolves_any_block() {
        assert_eq!(state_by_name("oak_stairs"), Some(2885));
        assert!(state_by_name("minecraft:diamond_block").is_some());
        assert_eq!(opacity(STONE), 15);
        assert_eq!(emission(5864), 15); // glowstone
    }
}
