//! World generation.
//!
//! Generators are plain functions of `(cx, cz) -> Chunk`, so they compose and
//! test easily. Two are provided: a superflat generator and a smooth
//! value-noise terrain generator that needs no external dependencies.

use crate::block::{self, StateId};
use crate::chunk::{Chunk, MIN_Y, SECTION_DIM};

/// Anything that can produce chunks on demand.
pub trait Generator: Send + Sync {
    /// Generate the chunk at chunk coordinates `(cx, cz)`.
    fn generate(&self, cx: i32, cz: i32) -> Chunk;

    /// A reasonable, solid-ground spawn Y for the column at world `(x, z)`.
    fn spawn_height(&self, x: i32, z: i32) -> i32;

    /// Short identifier surfaced in config/admin ("flat", "amplified", …).
    fn name(&self) -> &'static str;
}

/// Classic superflat: bedrock, dirt, grass at a fixed height.
pub struct FlatGenerator {
    pub surface_y: i32,
    pub top: StateId,
    pub filler: StateId,
}

impl Default for FlatGenerator {
    fn default() -> Self {
        FlatGenerator {
            surface_y: -60,
            top: block::GRASS_BLOCK,
            filler: block::DIRT,
        }
    }
}

impl Generator for FlatGenerator {
    fn generate(&self, cx: i32, cz: i32) -> Chunk {
        let mut chunk = Chunk::new(cx, cz);
        for x in 0..SECTION_DIM {
            for z in 0..SECTION_DIM {
                chunk.set_block(x, MIN_Y, z, block::BEDROCK);
                for y in (MIN_Y + 1)..self.surface_y {
                    chunk.set_block(x, y, z, self.filler);
                }
                chunk.set_block(x, self.surface_y, z, self.top);
            }
        }
        chunk
    }

    fn spawn_height(&self, _x: i32, _z: i32) -> i32 {
        self.surface_y + 1
    }

    fn name(&self) -> &'static str {
        "flat"
    }
}

/// Smooth rolling terrain built from layered value noise.
pub struct TerrainGenerator {
    pub seed: u64,
    pub base: i32,
    pub amplitude: f64,
    pub sea_level: i32,
}

impl Default for TerrainGenerator {
    fn default() -> Self {
        TerrainGenerator {
            seed: 0x5EED,
            base: 0,
            amplitude: 24.0,
            sea_level: 4,
        }
    }
}

impl TerrainGenerator {
    fn height_at(&self, wx: i32, wz: i32) -> i32 {
        // Two octaves of value noise for gentle hills.
        let n1 = value_noise(self.seed, wx as f64 / 48.0, wz as f64 / 48.0);
        let n2 = value_noise(self.seed ^ 0x9E37, wx as f64 / 16.0, wz as f64 / 16.0);
        let h = self.base as f64 + (n1 * 0.75 + n2 * 0.25) * self.amplitude;
        h.round() as i32
    }
}

impl TerrainGenerator {
    /// Rarely place a structure (surface hut or underground dungeon) per chunk.
    fn place_structures(&self, chunk: &mut Chunk, cx: i32, cz: i32) {
        let roll = hash_to_unit(self.seed ^ 0x57A2, cx as i64, cz as i64);
        if roll > 0.985 {
            // Surface hut around a fixed interior column.
            let (lx, lz) = (5usize, 5usize);
            let surface = self.height_at(cx * 16 + lx as i32, cz * 16 + lz as i32);
            if surface > self.sea_level {
                self.build_hut(chunk, lx, surface + 1, lz);
            }
        } else if roll < 0.012 {
            // Underground dungeon.
            let y = -24 + (hash_to_unit(self.seed ^ 0x0D, cx as i64, cz as i64) * 16.0) as i32;
            self.build_dungeon(chunk, 5, y, 5);
        }
    }

    fn build_hut(&self, chunk: &mut Chunk, x: usize, y: i32, z: usize) {
        let planks = block::OAK_PLANKS;
        let glass = block::GLASS;
        for dx in 0..5usize {
            for dz in 0..5usize {
                for dy in 0..4i32 {
                    let (bx, bz) = (x + dx, z + dz);
                    if bx >= 16 || bz >= 16 {
                        continue;
                    }
                    let edge = dx == 0 || dx == 4 || dz == 0 || dz == 4;
                    let block = if dy == 0 || dy == 3 {
                        planks // floor & roof
                    } else if edge {
                        // Walls, with a window in the middle of each side.
                        if (dx == 2 || dz == 2) && dy == 1 {
                            glass
                        } else {
                            planks
                        }
                    } else {
                        block::AIR
                    };
                    chunk.set_block(bx, y + dy, bz, block);
                }
            }
        }
        // Doorway.
        chunk.set_block(x, y + 1, z + 2, block::AIR);
        chunk.set_block(x, y + 2, z + 2, block::AIR);
    }

    fn build_dungeon(&self, chunk: &mut Chunk, x: usize, y: i32, z: usize) {
        let cobble = block::COBBLESTONE;
        let mossy = block::state_by_name("mossy_cobblestone").unwrap_or(cobble);
        let chest = block::CHEST;
        let spawner = block::state_by_name("spawner").unwrap_or(cobble);
        for dx in 0..5usize {
            for dz in 0..5usize {
                for dy in 0..4i32 {
                    let (bx, bz) = (x + dx, z + dz);
                    if bx >= 16 || bz >= 16 {
                        continue;
                    }
                    let shell = dx == 0 || dx == 4 || dz == 0 || dz == 4 || dy == 0 || dy == 3;
                    if shell {
                        let b = if dy == 0 && (dx + dz) % 2 == 0 { mossy } else { cobble };
                        chunk.set_block(bx, y + dy, bz, b);
                    } else {
                        chunk.set_block(bx, y + dy, bz, block::AIR);
                    }
                }
            }
        }
        // A spawner in the centre and a chest in a corner.
        if x + 2 < 16 && z + 2 < 16 {
            chunk.set_block(x + 2, y + 1, z + 2, spawner);
        }
        if x + 1 < 16 && z + 1 < 16 {
            chunk.set_block(x + 1, y + 1, z + 1, chest);
        }
    }
}

impl Generator for TerrainGenerator {
    fn generate(&self, cx: i32, cz: i32) -> Chunk {
        let mut chunk = Chunk::new(cx, cz);

        // Resolve ore / surface block ids once per chunk (full registry lookup).
        let coal = block::state_by_name("coal_ore").unwrap_or(block::STONE);
        let iron = block::state_by_name("iron_ore").unwrap_or(block::STONE);
        let gold = block::state_by_name("gold_ore").unwrap_or(block::STONE);
        let diamond = block::state_by_name("diamond_ore").unwrap_or(block::STONE);
        let snow = block::state_by_name("snow_block").unwrap_or(block::GRASS_BLOCK);

        for lx in 0..SECTION_DIM {
            for lz in 0..SECTION_DIM {
                let wx = cx * 16 + lx as i32;
                let wz = cz * 16 + lz as i32;
                let surface = self.height_at(wx, wz);
                // A slow temperature field drives surface "biomes".
                let temp = value_noise(self.seed ^ 0x7E33, wx as f64 / 160.0, wz as f64 / 160.0);

                chunk.set_block(lx, MIN_Y, lz, block::BEDROCK);
                for y in (MIN_Y + 1)..surface {
                    // Carve caves out of deep stone with 3D noise.
                    if y < surface - 3 && y > MIN_Y + 3 {
                        let cave = value_noise_3d(self.seed ^ 0xCA7E, wx as f64 / 16.0, y as f64 / 12.0, wz as f64 / 16.0);
                        if cave > 0.55 {
                            continue; // leave air
                        }
                    }
                    if y >= surface - 4 {
                        chunk.set_block(lx, y, lz, block::DIRT);
                        continue;
                    }
                    // Stone, with sparse depth-dependent ores.
                    let h = hash_to_unit(self.seed ^ 0x0E, wx as i64, (y as i64) << 8 | wz as i64);
                    let bucket = (h * 4096.0) as u32;
                    let ore = if y < -48 && bucket < 4 {
                        diamond
                    } else if y < 8 && bucket < 16 {
                        gold
                    } else if bucket < 64 {
                        iron
                    } else if bucket < 160 {
                        coal
                    } else {
                        block::STONE
                    };
                    chunk.set_block(lx, y, lz, ore);
                }

                // Surface block: sand at the shore, snow when cold, else grass.
                let top = if surface <= self.sea_level {
                    block::SAND
                } else if temp < -0.45 {
                    snow
                } else if temp > 0.5 {
                    block::SAND
                } else {
                    block::GRASS_BLOCK
                };
                chunk.set_block(lx, surface, lz, top);

                // Fill water up to sea level.
                for y in (surface + 1)..=self.sea_level {
                    chunk.set_block(lx, y, lz, block::WATER);
                }

                // Occasional natural tree on temperate grass (kept off chunk
                // edges so the canopy isn't clipped).
                if top == block::GRASS_BLOCK
                    && (2..14).contains(&lx)
                    && (2..14).contains(&lz)
                    && hash_to_unit(self.seed ^ 0x7EEE, wx as i64, wz as i64) > 0.992
                {
                    grow_tree(&mut chunk, lx, surface + 1, lz);
                }
            }
        }
        self.place_structures(&mut chunk, cx, cz);
        chunk
    }

    fn spawn_height(&self, x: i32, z: i32) -> i32 {
        self.height_at(x, z).max(self.sea_level) + 1
    }

    fn name(&self) -> &'static str {
        "terrain"
    }
}

/// Place a small oak tree (trunk + canopy) at a local column position.
fn grow_tree(chunk: &mut Chunk, x: usize, base_y: i32, z: usize) {
    let height = 4 + ((x + z) % 2) as i32;
    for i in 0..height {
        chunk.set_block(x, base_y + i, z, block::OAK_LOG);
    }
    for dy in (height - 2)..=(height + 1) {
        let r = if dy >= height { 1i32 } else { 2 };
        for dx in -r..=r {
            for dz in -r..=r {
                if dx == 0 && dz == 0 && dy < height {
                    continue;
                }
                let (lx, lz) = (x as i32 + dx, z as i32 + dz);
                if (0..16).contains(&lx) && (0..16).contains(&lz) {
                    chunk.set_block(lx as usize, base_y + dy, lz as usize, block::OAK_LEAVES);
                }
            }
        }
    }
}

/// Deterministic 3D value noise in roughly `[0, 1]` (for caves).
fn value_noise_3d(seed: u64, x: f64, y: f64, z: f64) -> f64 {
    let n = value_noise(seed, x, z) * 0.5 + value_noise(seed ^ 0xABCD, x + y * 0.7, z - y * 0.7) * 0.5;
    (n + 1.0) * 0.5
}

/// A simple Nether: bedrock shell, netherrack, a shallow lava sea.
pub struct NetherGenerator;

impl Generator for NetherGenerator {
    fn generate(&self, cx: i32, cz: i32) -> Chunk {
        let mut chunk = Chunk::new(cx, cz);
        let netherrack = block::state_by_name("netherrack").unwrap_or(block::STONE);
        let lava = block::state_by_name("lava").unwrap_or(block::WATER);
        for x in 0..SECTION_DIM {
            for z in 0..SECTION_DIM {
                chunk.set_block(x, MIN_Y, z, block::BEDROCK);
                for y in (MIN_Y + 1)..40 {
                    chunk.set_block(x, y, z, netherrack);
                }
                for y in (MIN_Y + 1)..6 {
                    chunk.set_block(x, y, z, lava);
                }
                chunk.set_block(x, 60, z, block::BEDROCK); // ceiling
            }
        }
        chunk
    }

    fn spawn_height(&self, _x: i32, _z: i32) -> i32 {
        42
    }

    fn name(&self) -> &'static str {
        "nether"
    }
}

/// A simple End: a central end-stone island floating in the void.
pub struct EndGenerator;

impl Generator for EndGenerator {
    fn generate(&self, cx: i32, cz: i32) -> Chunk {
        let mut chunk = Chunk::new(cx, cz);
        let end_stone = block::state_by_name("end_stone").unwrap_or(block::STONE);
        for x in 0..SECTION_DIM {
            for z in 0..SECTION_DIM {
                let wx = cx * 16 + x as i32;
                let wz = cz * 16 + z as i32;
                // ~40-block radius island around the origin.
                if (wx * wx + wz * wz) < 40 * 40 {
                    for y in 46..50 {
                        chunk.set_block(x, y, z, end_stone);
                    }
                }
            }
        }
        chunk
    }

    fn spawn_height(&self, _x: i32, _z: i32) -> i32 {
        50
    }

    fn name(&self) -> &'static str {
        "end"
    }
}

/// Deterministic 2D value noise in roughly `[-1, 1]`, smoothly interpolated.
fn value_noise(seed: u64, x: f64, z: f64) -> f64 {
    let x0 = x.floor() as i64;
    let z0 = z.floor() as i64;
    let fx = x - x0 as f64;
    let fz = z - z0 as f64;

    let v00 = hash_to_unit(seed, x0, z0);
    let v10 = hash_to_unit(seed, x0 + 1, z0);
    let v01 = hash_to_unit(seed, x0, z0 + 1);
    let v11 = hash_to_unit(seed, x0 + 1, z0 + 1);

    let sx = smoothstep(fx);
    let sz = smoothstep(fz);
    let a = lerp(v00, v10, sx);
    let b = lerp(v01, v11, sx);
    lerp(a, b, sz) * 2.0 - 1.0
}

fn hash_to_unit(seed: u64, x: i64, z: i64) -> f64 {
    // SplitMix64-style avalanche over the mixed coordinates.
    let mut h = seed
        ^ (x as u64).wrapping_mul(0x9E3779B97F4A7C15)
        ^ (z as u64).wrapping_mul(0xC2B2AE3D27D4EB4F);
    h ^= h >> 30;
    h = h.wrapping_mul(0xBF58476D1CE4E5B9);
    h ^= h >> 27;
    h = h.wrapping_mul(0x94D049BB133111EB);
    h ^= h >> 31;
    (h >> 11) as f64 / (1u64 << 53) as f64
}

fn smoothstep(t: f64) -> f64 {
    t * t * (3.0 - 2.0 * t)
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_has_grass_surface() {
        let g = FlatGenerator::default();
        let c = g.generate(0, 0);
        assert_eq!(c.get_block(0, g.surface_y, 0), block::GRASS_BLOCK);
        assert_eq!(c.get_block(0, MIN_Y, 0), block::BEDROCK);
    }

    #[test]
    fn terrain_is_deterministic() {
        let g = TerrainGenerator::default();
        assert_eq!(g.height_at(10, 20), g.height_at(10, 20));
        let c1 = g.generate(1, 1);
        let c2 = g.generate(1, 1);
        for y in [MIN_Y, 0, 5, 30] {
            assert_eq!(c1.get_block(2, y, 3), c2.get_block(2, y, 3));
        }
    }
}
