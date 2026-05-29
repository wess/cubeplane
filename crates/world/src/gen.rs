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

impl Generator for TerrainGenerator {
    fn generate(&self, cx: i32, cz: i32) -> Chunk {
        let mut chunk = Chunk::new(cx, cz);
        for lx in 0..SECTION_DIM {
            for lz in 0..SECTION_DIM {
                let wx = cx * 16 + lx as i32;
                let wz = cz * 16 + lz as i32;
                let surface = self.height_at(wx, wz);

                chunk.set_block(lx, MIN_Y, lz, block::BEDROCK);
                for y in (MIN_Y + 1)..surface {
                    // Stone underground, with a few blocks of dirt near the top.
                    let block = if y >= surface - 4 {
                        block::DIRT
                    } else {
                        block::STONE
                    };
                    chunk.set_block(lx, y, lz, block);
                }

                // Surface block depends on whether we're under the sea level.
                let top = if surface <= self.sea_level {
                    block::SAND
                } else {
                    block::GRASS_BLOCK
                };
                chunk.set_block(lx, surface, lz, top);

                // Fill water up to sea level.
                for y in (surface + 1)..=self.sea_level {
                    chunk.set_block(lx, y, lz, block::WATER);
                }
            }
        }
        chunk
    }

    fn spawn_height(&self, x: i32, z: i32) -> i32 {
        self.height_at(x, z).max(self.sea_level) + 1
    }

    fn name(&self) -> &'static str {
        "terrain"
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
