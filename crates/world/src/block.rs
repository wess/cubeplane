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
}

/// Whether a state id is air (and therefore not counted toward a section's
/// non-air block count, which the client uses to skip empty sections).
#[inline]
pub fn is_air(state: StateId) -> bool {
    state == AIR
}
