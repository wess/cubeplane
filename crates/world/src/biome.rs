//! Overworld biome definitions — the single source of truth shared by world
//! generation (which stamps a biome id into each chunk) and the server's Login
//! registry codec (which must advertise the very same ids and names). Keeping
//! both off one table guarantees the chunk biome ids always resolve in the
//! registry the client received.
//!
//! Each definition carries only the fields the 1.20.1 client validates on a
//! biome registry element: `has_precipitation`, `temperature`, `downfall` and
//! the four `effects` colours. The shapes mirror the values vanilla ships.

/// Coarse role of a biome, used by terrain shaping and mob spawning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BiomeKind {
    /// Deep water body.
    Ocean,
    /// Narrow water body threading the land.
    River,
    /// Sandy shoreline between water and land.
    Beach,
    /// Ordinary dry land (the climate is read from `temperature`).
    Land,
}

/// One biome's id, identifier and the climate/colour values the client needs.
#[derive(Debug, Clone, Copy)]
pub struct BiomeDef {
    pub id: u16,
    pub name: &'static str,
    pub kind: BiomeKind,
    pub temperature: f32,
    pub downfall: f32,
    pub has_precipitation: bool,
    pub sky_color: i32,
    pub water_color: i32,
}

// Stable network ids. `PLAINS` stays 1 to match `chunk::DEFAULT_BIOME`.
pub const OCEAN: u16 = 0;
pub const PLAINS: u16 = 1;
pub const DESERT: u16 = 2;
pub const FOREST: u16 = 3;
pub const TAIGA: u16 = 4;
pub const SWAMP: u16 = 5;
pub const RIVER: u16 = 6;
pub const SNOWY_PLAINS: u16 = 7;
pub const SNOWY_TAIGA: u16 = 8;
pub const SAVANNA: u16 = 9;
pub const JUNGLE: u16 = 10;
pub const BEACH: u16 = 11;
pub const WINDSWEPT_HILLS: u16 = 12;
pub const BIRCH_FOREST: u16 = 13;

/// Shared fog colours (constant across the set, as in vanilla overworld).
pub const FOG_COLOR: i32 = 0x00C0_D8FF;
pub const WATER_FOG_COLOR: i32 = 0x0005_0533;

#[allow(clippy::too_many_arguments)]
const fn def(
    id: u16,
    name: &'static str,
    kind: BiomeKind,
    temperature: f32,
    downfall: f32,
    has_precipitation: bool,
    sky_color: i32,
    water_color: i32,
) -> BiomeDef {
    BiomeDef { id, name, kind, temperature, downfall, has_precipitation, sky_color, water_color }
}

/// Every overworld biome cubeplane generates and advertises, ordered by id.
pub static BIOMES: &[BiomeDef] = &[
    def(OCEAN, "minecraft:ocean", BiomeKind::Ocean, 0.5, 0.5, true, 0x0078_A7FF, 0x003F_76E4),
    def(PLAINS, "minecraft:plains", BiomeKind::Land, 0.8, 0.4, true, 0x0078_A7FF, 0x003F_76E4),
    def(DESERT, "minecraft:desert", BiomeKind::Land, 2.0, 0.0, false, 0x0078_A7FF, 0x003F_76E4),
    def(FOREST, "minecraft:forest", BiomeKind::Land, 0.7, 0.8, true, 0x0079_A6FF, 0x003F_76E4),
    def(TAIGA, "minecraft:taiga", BiomeKind::Land, 0.25, 0.8, true, 0x008D_B7FF, 0x003F_76E4),
    def(SWAMP, "minecraft:swamp", BiomeKind::Land, 0.8, 0.9, true, 0x0078_A7FF, 0x0061_7B64),
    def(RIVER, "minecraft:river", BiomeKind::River, 0.5, 0.5, true, 0x0078_A7FF, 0x003F_76E4),
    def(SNOWY_PLAINS, "minecraft:snowy_plains", BiomeKind::Land, 0.0, 0.5, true, 0x008C_BED6, 0x003F_76E4),
    def(SNOWY_TAIGA, "minecraft:snowy_taiga", BiomeKind::Land, -0.5, 0.4, true, 0x0088_BBDF, 0x003D_57D6),
    def(SAVANNA, "minecraft:savanna", BiomeKind::Land, 2.0, 0.0, false, 0x0078_A7FF, 0x003F_76E4),
    def(JUNGLE, "minecraft:jungle", BiomeKind::Land, 0.95, 0.9, true, 0x0077_A8FF, 0x003F_76E4),
    def(BEACH, "minecraft:beach", BiomeKind::Beach, 0.8, 0.4, true, 0x0078_A7FF, 0x003F_76E4),
    def(WINDSWEPT_HILLS, "minecraft:windswept_hills", BiomeKind::Land, 0.2, 0.3, true, 0x0080_B6FF, 0x003F_76E4),
    def(BIRCH_FOREST, "minecraft:birch_forest", BiomeKind::Land, 0.6, 0.6, true, 0x0079_A6FF, 0x003F_76E4),
];

/// Look up a biome by network id.
pub fn by_id(id: u16) -> &'static BiomeDef {
    BIOMES.iter().find(|b| b.id == id).unwrap_or(&BIOMES[PLAINS as usize])
}

/// Whether a biome is cold enough that water freezes / snow falls.
pub fn is_snowy(id: u16) -> bool {
    by_id(id).temperature < 0.15
}

/// Whether a biome is open water (ocean or river).
pub fn is_water(id: u16) -> bool {
    matches!(by_id(id).kind, BiomeKind::Ocean | BiomeKind::River)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_contiguous_and_match_index() {
        for (i, b) in BIOMES.iter().enumerate() {
            assert_eq!(b.id as usize, i, "biome {} id must equal its index", b.name);
        }
    }

    #[test]
    fn plains_matches_default_biome() {
        assert_eq!(PLAINS, crate::chunk::DEFAULT_BIOME);
        assert_eq!(by_id(PLAINS).name, "minecraft:plains");
    }

    #[test]
    fn water_and_snow_classification() {
        assert!(is_water(OCEAN) && is_water(RIVER));
        assert!(!is_water(PLAINS));
        assert!(is_snowy(SNOWY_PLAINS) && is_snowy(SNOWY_TAIGA));
        assert!(!is_snowy(DESERT));
    }
}
