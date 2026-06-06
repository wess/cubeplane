//! Chunk storage and the wire encoding consumed by the Chunk Data packet.
//!
//! A chunk is a 16×384×16 column split into [`SECTION_COUNT`] vertical sections
//! of 16³ blocks. Each section serializes as a *paletted container* exactly as
//! the vanilla client expects, prefixed by its non-air block count.

use bytes::BytesMut;
use cubeplane_nbt::Nbt;
use cubeplane_protocol::ProtoWrite;

use crate::block::{self, StateId};

/// Lowest block Y coordinate in the overworld.
pub const MIN_Y: i32 = -64;
/// Total world height in blocks.
pub const WORLD_HEIGHT: i32 = 384;
/// Number of 16³ block sections in a column (`WORLD_HEIGHT / 16`).
pub const SECTION_COUNT: usize = (WORLD_HEIGHT / 16) as usize; // 24
/// Light is also stored for one section below and one above the world.
pub const LIGHT_SECTION_COUNT: usize = SECTION_COUNT + 2; // 26
/// Blocks per axis in a section.
pub const SECTION_DIM: usize = 16;
/// Blocks in a single section (`16³`).
pub const SECTION_VOLUME: usize = SECTION_DIM * SECTION_DIM * SECTION_DIM; // 4096
/// Default biome id; must match the registry index sent in Login (Play).
pub const DEFAULT_BIOME: u16 = 1; // "minecraft:plains" in our registry order

#[inline]
fn section_index(x: usize, y: usize, z: usize) -> usize {
    (y * SECTION_DIM + z) * SECTION_DIM + x
}

/// A 16³ block section. Stored densely; `None` means "all air" to save memory.
#[derive(Clone)]
pub struct Section {
    blocks: Option<Box<[StateId; SECTION_VOLUME]>>,
    biome: u16,
    non_air: u16,
}

impl Default for Section {
    fn default() -> Self {
        Section {
            blocks: None,
            biome: DEFAULT_BIOME,
            non_air: 0,
        }
    }
}

impl Section {
    /// Read a block within the section (local 0..16 coords).
    pub fn get(&self, x: usize, y: usize, z: usize) -> StateId {
        match &self.blocks {
            Some(b) => b[section_index(x, y, z)],
            None => block::AIR,
        }
    }

    /// Set a block within the section, maintaining the non-air count.
    pub fn set(&mut self, x: usize, y: usize, z: usize, state: StateId) {
        let idx = section_index(x, y, z);
        let blocks = self
            .blocks
            .get_or_insert_with(|| Box::new([block::AIR; SECTION_VOLUME]));
        let prev = blocks[idx];
        if !block::is_air(prev) {
            self.non_air -= 1;
        }
        if !block::is_air(state) {
            self.non_air += 1;
        }
        blocks[idx] = state;
        if self.non_air == 0 {
            self.blocks = None;
        }
    }

    fn is_empty(&self) -> bool {
        self.non_air == 0
    }

    /// Set the (single) biome for this section.
    pub fn set_biome(&mut self, biome: u16) {
        self.biome = biome;
    }

    /// This section's biome id.
    pub fn biome(&self) -> u16 {
        self.biome
    }

    /// Serialize this section as `[block count: short][block states][biomes]`.
    fn encode(&self, buf: &mut BytesMut) {
        buf.write_i16(self.non_air as i16);
        match &self.blocks {
            None => encode_single_valued(buf, block::AIR as u32),
            Some(b) => encode_block_palette(buf, b.as_ref()),
        }
        // Biomes: single-valued container (one biome per section is plenty).
        encode_single_valued(buf, self.biome as u32);
    }
}

/// Write a single-valued paletted container: 0 bits-per-entry, one palette
/// value, empty data array.
fn encode_single_valued(buf: &mut BytesMut, value: u32) {
    buf.write_u8(0); // bits per entry
    buf.write_varint(value as i32); // palette = the one value
    buf.write_varint(0); // data array length (longs)
}

/// Write a block-states paletted container using the smallest representation:
/// single-valued, indirect (4–8 bits) or direct (15 bit global ids).
fn encode_block_palette(buf: &mut BytesMut, blocks: &[StateId; SECTION_VOLUME]) {
    // Build the distinct palette.
    let mut palette: Vec<StateId> = Vec::new();
    for &b in blocks.iter() {
        if !palette.contains(&b) {
            palette.push(b);
        }
    }

    if palette.len() == 1 {
        encode_single_valued(buf, palette[0] as u32);
        return;
    }

    let palette_bits = bits_for(palette.len());
    if palette_bits <= 8 {
        let bits = palette_bits.max(4);
        buf.write_u8(bits as u8);
        buf.write_varint(palette.len() as i32);
        for &state in &palette {
            buf.write_varint(state as i32);
        }
        let indices: Vec<u32> = blocks
            .iter()
            .map(|b| palette.iter().position(|&p| p == *b).unwrap() as u32)
            .collect();
        write_packed(buf, &indices, bits);
    } else {
        // Direct palette: 15-bit global ids, no palette section.
        let bits = 15;
        buf.write_u8(bits as u8);
        let indices: Vec<u32> = blocks.iter().map(|&b| b as u32).collect();
        write_packed(buf, &indices, bits);
    }
}

/// Bits required to index `len` distinct values (minimum 1).
fn bits_for(len: usize) -> usize {
    if len <= 1 {
        return 0;
    }
    let mut bits = 0;
    let mut max = 1;
    while max < len {
        max <<= 1;
        bits += 1;
    }
    bits
}

/// Pack `values` into i64s, `bits` per value, without spanning long boundaries
/// (the compact format used since Minecraft 1.16).
fn write_packed(buf: &mut BytesMut, values: &[u32], bits: usize) {
    let per_long = 64 / bits;
    let long_count = values.len().div_ceil(per_long);
    buf.write_varint(long_count as i32);
    let mut idx = 0;
    for _ in 0..long_count {
        let mut cur: u64 = 0;
        for slot in 0..per_long {
            if idx >= values.len() {
                break;
            }
            cur |= (values[idx] as u64) << (slot * bits);
            idx += 1;
        }
        buf.write_i64(cur as i64);
    }
}

/// A full 16×384×16 chunk column at chunk coordinates `(cx, cz)`.
#[derive(Clone)]
pub struct Chunk {
    pub cx: i32,
    pub cz: i32,
    sections: Vec<Section>,
    /// Cached lighting, invalidated on edit and recomputed lazily.
    light: Option<LightData>,
}

impl Chunk {
    /// Cached lighting, if computed since the last edit.
    pub fn cached_light(&self) -> Option<&LightData> {
        self.light.as_ref()
    }

    /// Store computed lighting for reuse on subsequent sends.
    pub fn set_light(&mut self, light: LightData) {
        self.light = Some(light);
    }

    /// Create an all-air chunk at the given chunk coordinates.
    pub fn new(cx: i32, cz: i32) -> Self {
        Chunk {
            cx,
            cz,
            light: None,
            sections: vec![Section::default(); SECTION_COUNT],
        }
    }

    /// Read a block at world Y (`MIN_Y..MIN_Y+WORLD_HEIGHT`) and local x/z.
    pub fn get_block(&self, x: usize, y: i32, z: usize) -> StateId {
        match self.section_of(y) {
            Some((s, ly)) => self.sections[s].get(x, ly, z),
            None => block::AIR,
        }
    }

    /// Place a block at world Y and local x/z. Out-of-range Y is ignored.
    pub fn set_block(&mut self, x: usize, y: i32, z: usize, state: StateId) {
        if let Some((s, ly)) = self.section_of(y) {
            self.sections[s].set(x, ly, z, state);
            self.light = None; // invalidate cached lighting
        }
    }

    /// Stamp one biome across the whole column (every vertical section). The
    /// overworld's biome is horizontal, so a single value per column is enough.
    pub fn set_biome(&mut self, biome: u16) {
        for s in &mut self.sections {
            s.set_biome(biome);
        }
    }

    /// The column's biome id (read from the lowest section; all share it).
    pub fn biome(&self) -> u16 {
        self.sections.first().map(Section::biome).unwrap_or(DEFAULT_BIOME)
    }

    fn section_of(&self, y: i32) -> Option<(usize, usize)> {
        if !(MIN_Y..MIN_Y + WORLD_HEIGHT).contains(&y) {
            return None;
        }
        let rel = (y - MIN_Y) as usize;
        Some((rel / SECTION_DIM, rel % SECTION_DIM))
    }

    /// The `MOTION_BLOCKING` heightmap NBT the client expects in Chunk Data.
    pub fn heightmaps(&self) -> Nbt {
        // 9 bits is enough for heights in 0..=384.
        let bits = 9;
        let mut heights = vec![0u32; 256];
        for z in 0..SECTION_DIM {
            for x in 0..SECTION_DIM {
                let mut h = 0u32;
                for y in (MIN_Y..MIN_Y + WORLD_HEIGHT).rev() {
                    if !block::is_air(self.get_block(x, y, z)) {
                        // Heightmap stores height above the world bottom.
                        h = (y - MIN_Y + 1) as u32;
                        break;
                    }
                }
                heights[z * SECTION_DIM + x] = h;
            }
        }
        let packed = pack_to_longs(&heights, bits);
        Nbt::compound()
            .put_long_array("MOTION_BLOCKING", packed.clone())
            .put_long_array("WORLD_SURFACE", packed)
    }

    /// Serialize the section column for the Chunk Data packet's `Data` field.
    pub fn encode_sections(&self) -> BytesMut {
        let mut buf = BytesMut::new();
        for section in &self.sections {
            section.encode(&mut buf);
        }
        buf
    }

    /// Compute real sky and block lighting for this chunk via flood-fill.
    ///
    /// Skylight starts at 15 in open columns and floods down/around losing one
    /// level per step; block light floods out from emissive blocks. The BFS is
    /// confined to the chunk (cross-chunk seams are not resolved), which keeps
    /// it cheap while producing correct surface lighting and lit caves/torches.
    pub fn compute_light(&self) -> LightData {
        let h = WORLD_HEIGHT as usize;
        let vol = SECTION_DIM * h * SECTION_DIM;
        let cell = |x: usize, y: usize, z: usize| (y * SECTION_DIM + z) * SECTION_DIM + x;

        let mut sky = vec![0u8; vol];
        let mut blk = vec![0u8; vol];

        // Direct skylight: light from the top until the first opaque block.
        for x in 0..SECTION_DIM {
            for z in 0..SECTION_DIM {
                let mut open = true;
                for yy in (0..h).rev() {
                    let state = self.get_block(x, MIN_Y + yy as i32, z);
                    if open && block::opacity(state) == 0 {
                        sky[cell(x, yy, z)] = 15;
                    } else {
                        open = false;
                    }
                }
            }
        }

        // Seed block light from emissive blocks.
        for x in 0..SECTION_DIM {
            for z in 0..SECTION_DIM {
                for yy in 0..h {
                    let e = block::emission(self.get_block(x, MIN_Y + yy as i32, z));
                    if e > 0 {
                        blk[cell(x, yy, z)] = e;
                    }
                }
            }
        }

        self.flood(&mut sky, vol, &cell);
        self.flood(&mut blk, vol, &cell);

        // Pack into per-section 2048-byte arrays (one nibble per block).
        let mut sky_arrays = Vec::with_capacity(LIGHT_SECTION_COUNT);
        let mut blk_arrays = Vec::with_capacity(LIGHT_SECTION_COUNT);
        let mut sky_mask = 0u64;
        let mut blk_mask = 0u64;
        let mut empty_blk_mask = 0u64;

        for ls in 0..LIGHT_SECTION_COUNT {
            // ls 0 is below the world, ls 25 is above it; 1..=24 are sections.
            let (sky_sec, blk_sec) = if ls == 0 {
                (vec![0u8; 2048], vec![0u8; 2048])
            } else if ls == LIGHT_SECTION_COUNT - 1 {
                (vec![0xFFu8; 2048], vec![0u8; 2048]) // open sky above
            } else {
                let base = (ls - 1) * SECTION_DIM;
                let mut s = vec![0u8; 2048];
                let mut b = vec![0u8; 2048];
                for c in 0..SECTION_VOLUME {
                    let (lx, lz, ly) = (c & 15, (c >> 4) & 15, c >> 8);
                    let world = cell(lx, base + ly, lz);
                    put_nibble(&mut s, c, sky[world]);
                    put_nibble(&mut b, c, blk[world]);
                }
                (s, b)
            };
            sky_mask |= 1 << ls;
            sky_arrays.push(sky_sec);
            if blk_sec.iter().any(|&v| v != 0) {
                blk_mask |= 1 << ls;
                blk_arrays.push(blk_sec);
            } else {
                empty_blk_mask |= 1 << ls;
            }
        }

        LightData {
            sky_light_mask: sky_mask,
            block_light_mask: blk_mask,
            empty_sky_light_mask: 0,
            empty_block_light_mask: empty_blk_mask,
            sky_light: sky_arrays,
            block_light: blk_arrays,
        }
    }

    /// Breadth-first light propagation over a light grid.
    fn flood(&self, light: &mut [u8], vol: usize, cell: &impl Fn(usize, usize, usize) -> usize) {
        use std::collections::VecDeque;
        let h = WORLD_HEIGHT as usize;
        let mut queue: VecDeque<usize> = (0..vol).filter(|&i| light[i] > 1).collect();
        while let Some(i) = queue.pop_front() {
            let level = light[i];
            if level <= 1 {
                continue;
            }
            let (x, z, y) = (i & 15, (i >> 4) & 15, i >> 8);
            let mut spread = |nx: usize, ny: usize, nz: usize, queue: &mut VecDeque<usize>| {
                let state = self.get_block(nx, MIN_Y + ny as i32, nz);
                let op = block::opacity(state);
                if op >= 15 {
                    return; // opaque: light stops
                }
                let n = cell(nx, ny, nz);
                let next = level.saturating_sub(op.max(1));
                if next > light[n] {
                    light[n] = next;
                    if next > 1 {
                        queue.push_back(n);
                    }
                }
            };
            if x > 0 { spread(x - 1, y, z, &mut queue); }
            if x < SECTION_DIM - 1 { spread(x + 1, y, z, &mut queue); }
            if z > 0 { spread(x, y, z - 1, &mut queue); }
            if z < SECTION_DIM - 1 { spread(x, y, z + 1, &mut queue); }
            if y > 0 { spread(x, y - 1, z, &mut queue); }
            if y < h - 1 { spread(x, y + 1, z, &mut queue); }
        }
    }

    /// Whether every section is empty (all air) — lets callers skip work.
    pub fn is_empty(&self) -> bool {
        self.sections.iter().all(Section::is_empty)
    }

    /// Flatten the column to a grid of state ids in `((y_rel*16)+z)*16+x` order
    /// (`y_rel = world_y - MIN_Y`), for full-chunk persistence.
    pub fn to_grid(&self) -> Vec<StateId> {
        let h = WORLD_HEIGHT as usize;
        let mut g = vec![0u16; h * 256];
        for yr in 0..h {
            for z in 0..SECTION_DIM {
                for x in 0..SECTION_DIM {
                    g[(yr * 16 + z) * 16 + x] = self.get_block(x, MIN_Y + yr as i32, z);
                }
            }
        }
        g
    }

    /// Rebuild a chunk from a flattened grid produced by [`Chunk::to_grid`].
    pub fn from_grid(cx: i32, cz: i32, grid: &[StateId]) -> Chunk {
        let mut c = Chunk::new(cx, cz);
        let h = WORLD_HEIGHT as usize;
        for yr in 0..h {
            for z in 0..SECTION_DIM {
                for x in 0..SECTION_DIM {
                    let s = grid[(yr * 16 + z) * 16 + x];
                    if !block::is_air(s) {
                        c.set_block(x, MIN_Y + yr as i32, z, s);
                    }
                }
            }
        }
        c
    }
}

/// Pre-computed light payload for the Chunk Data / Update Light packets.
#[derive(Clone)]
pub struct LightData {
    pub sky_light_mask: u64,
    pub block_light_mask: u64,
    pub empty_sky_light_mask: u64,
    pub empty_block_light_mask: u64,
    pub sky_light: Vec<Vec<u8>>,
    pub block_light: Vec<Vec<u8>>,
}

/// Set the 4-bit light value for cell `c` in a 2048-byte section array.
fn put_nibble(arr: &mut [u8], c: usize, v: u8) {
    let b = c / 2;
    if c & 1 == 0 {
        arr[b] = (arr[b] & 0xF0) | (v & 0x0F);
    } else {
        arr[b] = (arr[b] & 0x0F) | ((v & 0x0F) << 4);
    }
}

/// Pack `values` (each `bits` wide) into i64s without spanning longs.
fn pack_to_longs(values: &[u32], bits: usize) -> Vec<i64> {
    let per_long = 64 / bits;
    let long_count = values.len().div_ceil(per_long);
    let mut out = Vec::with_capacity(long_count);
    let mut idx = 0;
    for _ in 0..long_count {
        let mut cur: u64 = 0;
        for slot in 0..per_long {
            if idx >= values.len() {
                break;
            }
            cur |= (values[idx] as u64) << (slot * bits);
            idx += 1;
        }
        out.push(cur as i64);
    }
    out
}

/// Helper to write a Minecraft `BitSet` (VarInt long count + longs).
pub fn write_bitset(buf: &mut BytesMut, mask: u64) {
    if mask == 0 {
        buf.write_varint(0);
    } else {
        buf.write_varint(1);
        buf.write_i64(mask as i64);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cubeplane_nbt::Value;

    #[test]
    fn set_get_block() {
        let mut c = Chunk::new(0, 0);
        c.set_block(3, 70, 5, block::STONE);
        assert_eq!(c.get_block(3, 70, 5), block::STONE);
        assert_eq!(c.get_block(3, 71, 5), block::AIR);
        c.set_block(3, 70, 5, block::AIR);
        assert_eq!(c.get_block(3, 70, 5), block::AIR);
    }

    #[test]
    fn heightmap_tracks_surface() {
        let mut c = Chunk::new(0, 0);
        for y in MIN_Y..=0 {
            c.set_block(0, y, 0, block::STONE);
        }
        // Surface block is at y=0, height above bottom = 0 - (-64) + 1 = 65.
        let hm = c.heightmaps();
        match hm.into_value() {
            Value::Compound(m) => match m.get("MOTION_BLOCKING").unwrap() {
                Value::LongArray(longs) => {
                    let first = longs[0] as u64 & ((1 << 9) - 1);
                    assert_eq!(first, 65);
                }
                _ => panic!("wrong tag"),
            },
            _ => panic!("not compound"),
        }
    }

    #[test]
    fn encodes_without_panicking() {
        let mut c = Chunk::new(0, 0);
        for x in 0..16 {
            for z in 0..16 {
                c.set_block(x, -64, z, block::BEDROCK);
                c.set_block(x, -63, z, block::DIRT);
                c.set_block(x, -62, z, block::GRASS_BLOCK);
            }
        }
        let data = c.encode_sections();
        assert!(!data.is_empty());
        let light = c.compute_light();
        assert_eq!(light.sky_light.len(), LIGHT_SECTION_COUNT);
    }

    #[test]
    fn lighting_is_dark_under_cover_and_lit_above() {
        let mut c = Chunk::new(0, 0);
        // Solid stone ceiling at y=0 over the whole chunk.
        for x in 0..SECTION_DIM {
            for z in 0..SECTION_DIM {
                c.set_block(x, 0, z, block::STONE);
            }
        }
        let light = c.compute_light();
        // Sky sections exist for all light sections.
        assert_eq!(light.sky_light.len(), LIGHT_SECTION_COUNT);
        // A torch lights its surroundings (block light section becomes non-empty).
        let mut lit = Chunk::new(0, 0);
        lit.set_block(8, 70, 8, block::CHEST); // any block
        lit.set_block(8, 71, 8, 5864); // glowstone (emission 15)
        let l2 = lit.compute_light();
        assert!(!l2.block_light.is_empty(), "glowstone should emit block light");
    }
}
