//! # cubeplane-world
//!
//! Block registry, chunk storage and world generation. The [`World`] lazily
//! generates chunks through a pluggable [`Generator`] and exposes global
//! block get/set in world coordinates, which the server and mod API build on.

pub mod block;
pub mod blocks_table;
pub mod chunk;
pub mod gen;

use std::collections::HashMap;
use std::sync::Arc;

pub use block::StateId;
pub use chunk::{Chunk, MIN_Y, SECTION_COUNT, WORLD_HEIGHT};
pub use gen::{FlatGenerator, Generator, TerrainGenerator};

/// A lazily-generated, in-memory world.
pub struct World {
    generator: Arc<dyn Generator>,
    chunks: HashMap<(i32, i32), Chunk>,
    /// Player/mod block edits over the generator, persisted to disk. Applied to
    /// chunks as they are (re)generated so the world survives reloads and chunk
    /// unloading.
    edits: HashMap<(i32, i32, i32), StateId>,
    spawn: (f64, f64, f64),
}

impl World {
    /// Build a world from a generator, computing a spawn point above the
    /// surface at the origin column.
    pub fn new(generator: Arc<dyn Generator>) -> Self {
        let spawn_y = generator.spawn_height(0, 0) as f64;
        World {
            generator,
            chunks: HashMap::new(),
            edits: HashMap::new(),
            spawn: (0.5, spawn_y, 0.5),
        }
    }

    /// Seed the world with previously-saved block edits (call before serving).
    pub fn load_edits(&mut self, edits: HashMap<(i32, i32, i32), StateId>) {
        self.edits = edits;
    }

    /// The persisted block edits, for saving.
    pub fn edits(&self) -> &HashMap<(i32, i32, i32), StateId> {
        &self.edits
    }

    /// Generate a chunk and overlay any saved edits that fall within it.
    fn generate_chunk(&self, cx: i32, cz: i32) -> Chunk {
        let mut chunk = self.generator.generate(cx, cz);
        if !self.edits.is_empty() {
            for (&(x, y, z), &state) in &self.edits {
                if x.div_euclid(16) == cx && z.div_euclid(16) == cz {
                    chunk.set_block(x.rem_euclid(16) as usize, y, z.rem_euclid(16) as usize, state);
                }
            }
        }
        chunk
    }

    /// Unload a chunk from memory (its edits persist in the edit map).
    pub fn unload_chunk(&mut self, cx: i32, cz: i32) -> bool {
        self.chunks.remove(&(cx, cz)).is_some()
    }

    /// The world spawn point `(x, y, z)`.
    pub fn spawn(&self) -> (f64, f64, f64) {
        self.spawn
    }

    /// Name of the active generator.
    pub fn generator_name(&self) -> &'static str {
        self.generator.name()
    }

    /// Number of currently-loaded chunks.
    pub fn loaded_chunks(&self) -> usize {
        self.chunks.len()
    }

    /// Get a chunk, generating and caching it if not yet loaded.
    pub fn chunk(&mut self, cx: i32, cz: i32) -> &Chunk {
        if !self.chunks.contains_key(&(cx, cz)) {
            let chunk = self.generate_chunk(cx, cz);
            self.chunks.insert((cx, cz), chunk);
        }
        self.chunks.get(&(cx, cz)).unwrap()
    }

    /// Whether a chunk is already generated/loaded.
    pub fn is_loaded(&self, cx: i32, cz: i32) -> bool {
        self.chunks.contains_key(&(cx, cz))
    }

    /// Drop every loaded chunk not in `keep` from memory, returning the number
    /// unloaded. Edits persist in the edit map, so unloaded chunks regenerate
    /// identically on demand.
    pub fn retain_chunks(&mut self, keep: &std::collections::HashSet<(i32, i32)>) -> usize {
        let before = self.chunks.len();
        self.chunks.retain(|coord, _| keep.contains(coord));
        before - self.chunks.len()
    }

    /// Read a block at world coordinates, generating the chunk if needed.
    pub fn get_block(&mut self, x: i32, y: i32, z: i32) -> StateId {
        let (cx, cz, lx, lz) = world_to_chunk(x, z);
        self.chunk(cx, cz).get_block(lx, y, lz)
    }

    /// Place a block at world coordinates, generating the chunk if needed.
    /// Records the edit for persistence and returns the affected chunk coords.
    pub fn set_block(&mut self, x: i32, y: i32, z: i32, state: StateId) -> (i32, i32) {
        let (cx, cz, lx, lz) = world_to_chunk(x, z);
        if !self.chunks.contains_key(&(cx, cz)) {
            let chunk = self.generate_chunk(cx, cz);
            self.chunks.insert((cx, cz), chunk);
        }
        self.chunks.get_mut(&(cx, cz)).unwrap().set_block(lx, y, lz, state);
        self.edits.insert((x, y, z), state);
        (cx, cz)
    }
}

/// Decompose world `(x, z)` into `(chunk_x, chunk_z, local_x, local_z)`.
pub fn world_to_chunk(x: i32, z: i32) -> (i32, i32, usize, usize) {
    let cx = x.div_euclid(16);
    let cz = z.div_euclid(16);
    let lx = x.rem_euclid(16) as usize;
    let lz = z.rem_euclid(16) as usize;
    (cx, cz, lx, lz)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_to_chunk_negative() {
        assert_eq!(world_to_chunk(-1, -1), (-1, -1, 15, 15));
        assert_eq!(world_to_chunk(0, 0), (0, 0, 0, 0));
        assert_eq!(world_to_chunk(17, -17), (1, -2, 1, 15));
    }

    #[test]
    fn lazily_generates_and_caches() {
        let mut w = World::new(Arc::new(FlatGenerator::default()));
        assert_eq!(w.loaded_chunks(), 0);
        let _ = w.get_block(5, -60, 5);
        assert_eq!(w.loaded_chunks(), 1);
        assert!(w.is_loaded(0, 0));
        // Setting a block in a fresh chunk reports its coords.
        assert_eq!(w.set_block(100, 10, 100, block::STONE), (6, 6));
    }
}
