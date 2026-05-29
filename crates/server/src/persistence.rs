//! On-disk persistence: world block edits and per-player data.
//!
//! cubeplane does not implement the Anvil region format. Instead it persists
//! the *delta* over the deterministic generator — every block a player, mob or
//! mod changed — plus a small JSON file per player. This survives restarts and
//! chunk unloading without the complexity of full chunk serialization.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

const BLOCKS_FILE: &str = "blocks.bin";
const BLOCKS_MAGIC: &[u8; 4] = b"CPB1";

/// Saved per-player state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerData {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
    pub health: f32,
    pub food: i32,
    pub saturation: f32,
    pub xp_total: i32,
    /// `(slot, item id, count)` for non-empty inventory slots.
    pub items: Vec<(u16, i32, u8)>,
}

fn blocks_path(dir: &Path) -> PathBuf {
    dir.join(BLOCKS_FILE)
}

fn players_dir(dir: &Path) -> PathBuf {
    dir.join("players")
}

fn player_path(dir: &Path, uuid: Uuid) -> PathBuf {
    players_dir(dir).join(format!("{uuid}.json"))
}

/// Load block edits from disk; an absent file yields an empty map.
pub fn load_blocks(dir: &Path) -> HashMap<(i32, i32, i32), u16> {
    let mut map = HashMap::new();
    let path = blocks_path(dir);
    let Ok(mut file) = std::fs::File::open(&path) else {
        return map;
    };
    let mut buf = Vec::new();
    if file.read_to_end(&mut buf).is_err() || buf.len() < 8 || &buf[0..4] != BLOCKS_MAGIC {
        return map;
    }
    let count = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]) as usize;
    let mut off = 8;
    for _ in 0..count {
        if off + 14 > buf.len() {
            break;
        }
        let x = i32::from_le_bytes(buf[off..off + 4].try_into().unwrap());
        let y = i32::from_le_bytes(buf[off + 4..off + 8].try_into().unwrap());
        let z = i32::from_le_bytes(buf[off + 8..off + 12].try_into().unwrap());
        let s = u16::from_le_bytes(buf[off + 12..off + 14].try_into().unwrap());
        map.insert((x, y, z), s);
        off += 14;
    }
    map
}

/// Persist block edits, written atomically via a temp file + rename.
pub fn save_blocks(dir: &Path, edits: &HashMap<(i32, i32, i32), u16>) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut buf = Vec::with_capacity(8 + edits.len() * 14);
    buf.extend_from_slice(BLOCKS_MAGIC);
    buf.extend_from_slice(&(edits.len() as u32).to_le_bytes());
    for (&(x, y, z), &s) in edits {
        buf.extend_from_slice(&x.to_le_bytes());
        buf.extend_from_slice(&y.to_le_bytes());
        buf.extend_from_slice(&z.to_le_bytes());
        buf.extend_from_slice(&s.to_le_bytes());
    }
    let tmp = blocks_path(dir).with_extension("tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(&buf)?;
        f.sync_all().ok();
    }
    std::fs::rename(tmp, blocks_path(dir))
}

/// A container entry: position and 27 `(item id, count)` pairs.
type ContainerEntry = ((i32, i32, i32), Vec<(i32, u8)>);

fn containers_path(dir: &Path) -> PathBuf {
    dir.join("containers.json")
}

/// Persist chest contents.
pub fn save_containers(dir: &Path, entries: &[ContainerEntry]) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let json = serde_json::to_string(entries).map_err(io::Error::other)?;
    std::fs::write(containers_path(dir), json)
}

/// Load chest contents (empty if absent).
pub fn load_containers(dir: &Path) -> Vec<ContainerEntry> {
    std::fs::read_to_string(containers_path(dir))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// World metadata persisted alongside chunks (time of day, etc.).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorldMeta {
    pub time: i64,
}

/// Live entities persisted across restarts.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EntitySave {
    /// `(mob name, x, y, z, yaw, health)`.
    pub mobs: Vec<(String, f64, f64, f64, f32, f32)>,
    /// `(type id, x, y, z, yaw)`.
    pub vehicles: Vec<(i32, f64, f64, f64, f32)>,
    /// `(item id, count, x, y, z)`.
    pub items: Vec<(i32, u8, f64, f64, f64)>,
}

fn meta_path(dir: &Path) -> PathBuf {
    dir.join("world_meta.json")
}

fn entities_path(dir: &Path) -> PathBuf {
    dir.join("entities.json")
}

pub fn save_meta(dir: &Path, meta: &WorldMeta) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(meta_path(dir), serde_json::to_string(meta).map_err(io::Error::other)?)
}

pub fn load_meta(dir: &Path) -> WorldMeta {
    std::fs::read_to_string(meta_path(dir))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save_entities(dir: &Path, ents: &EntitySave) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(entities_path(dir), serde_json::to_string(ents).map_err(io::Error::other)?)
}

pub fn load_entities(dir: &Path) -> EntitySave {
    std::fs::read_to_string(entities_path(dir))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Load a player's saved data, if any.
pub fn load_player(dir: &Path, uuid: Uuid) -> Option<PlayerData> {
    let text = std::fs::read_to_string(player_path(dir, uuid)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Persist a player's data.
pub fn save_player(dir: &Path, uuid: Uuid, data: &PlayerData) -> io::Result<()> {
    std::fs::create_dir_all(players_dir(dir))?;
    let json = serde_json::to_string_pretty(data).map_err(io::Error::other)?;
    std::fs::write(player_path(dir, uuid), json)
}

/// Full-chunk persistence: each chunk's complete block grid is run-length
/// encoded to a file `region/c.<cx>.<cz>.bin`. This stores entire chunks rather
/// than a delta over the generator — the moral equivalent of Anvil regions.
pub struct RegionStore {
    dir: PathBuf,
}

const REGION_MAGIC: &[u8; 4] = b"CPR1";

impl RegionStore {
    pub fn new(dir: &Path) -> RegionStore {
        let dir = dir.join("region");
        let _ = std::fs::create_dir_all(&dir);
        RegionStore { dir }
    }

    fn chunk_path(&self, cx: i32, cz: i32) -> PathBuf {
        self.dir.join(format!("c.{cx}.{cz}.bin"))
    }

    /// Save a chunk's flat block grid (run-length encoded).
    pub fn save_chunk(&self, cx: i32, cz: i32, grid: &[u16]) -> io::Result<()> {
        let mut buf = Vec::new();
        buf.extend_from_slice(REGION_MAGIC);
        // RLE: pairs of (state u16, run-length u32).
        let mut i = 0;
        while i < grid.len() {
            let v = grid[i];
            let mut run = 1u32;
            while i + (run as usize) < grid.len() && grid[i + run as usize] == v {
                run += 1;
            }
            buf.extend_from_slice(&v.to_le_bytes());
            buf.extend_from_slice(&run.to_le_bytes());
            i += run as usize;
        }
        let tmp = self.chunk_path(cx, cz).with_extension("tmp");
        std::fs::write(&tmp, &buf)?;
        std::fs::rename(tmp, self.chunk_path(cx, cz))
    }

    /// Load a chunk's flat block grid, if it was saved.
    pub fn load_chunk(&self, cx: i32, cz: i32) -> Option<Vec<u16>> {
        let buf = std::fs::read(self.chunk_path(cx, cz)).ok()?;
        if buf.len() < 4 || &buf[0..4] != REGION_MAGIC {
            return None;
        }
        let mut grid = Vec::new();
        let mut off = 4;
        while off + 6 <= buf.len() {
            let v = u16::from_le_bytes([buf[off], buf[off + 1]]);
            let run = u32::from_le_bytes([buf[off + 2], buf[off + 3], buf[off + 4], buf[off + 5]]);
            for _ in 0..run {
                grid.push(v);
            }
            off += 6;
        }
        Some(grid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_roundtrip() {
        let dir = std::env::temp_dir().join(format!("cp-persist-{}", std::process::id()));
        let mut edits = HashMap::new();
        edits.insert((1, -60, 2), 9u16);
        edits.insert((-30, 5, 700), 1u16);
        save_blocks(&dir, &edits).unwrap();
        let loaded = load_blocks(&dir);
        assert_eq!(loaded, edits);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn region_chunk_roundtrip() {
        let dir = std::env::temp_dir().join(format!("cp-region-{}", std::process::id()));
        let store = RegionStore::new(&dir);
        let mut grid = vec![0u16; 100];
        grid[10] = 1;
        grid[11] = 1;
        grid[50] = 80;
        store.save_chunk(3, -4, &grid).unwrap();
        assert_eq!(store.load_chunk(3, -4), Some(grid));
        assert_eq!(store.load_chunk(0, 0), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn meta_and_entities_roundtrip() {
        let dir = std::env::temp_dir().join(format!("cp-ent-{}", std::process::id()));
        save_meta(&dir, &WorldMeta { time: 13500 }).unwrap();
        assert_eq!(load_meta(&dir).time, 13500);

        let ents = EntitySave {
            mobs: vec![("villager".into(), 1.0, 64.0, 2.0, 90.0, 18.0)],
            vehicles: vec![(9, 3.0, 64.0, 4.0, 0.0)],
            items: vec![(1, 5, 0.0, 64.0, 0.0)],
        };
        save_entities(&dir, &ents).unwrap();
        let loaded = load_entities(&dir);
        assert_eq!(loaded.mobs.len(), 1);
        assert_eq!(loaded.mobs[0].0, "villager");
        assert_eq!(loaded.vehicles[0].0, 9);
        assert_eq!(loaded.items[0], (1, 5, 0.0, 64.0, 0.0));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn player_roundtrip() {
        let dir = std::env::temp_dir().join(format!("cp-persist-p-{}", std::process::id()));
        let uuid = Uuid::nil();
        let data = PlayerData {
            x: 1.0, y: 64.0, z: 2.0, yaw: 0.0, pitch: 0.0,
            health: 18.0, food: 17, saturation: 4.0, xp_total: 30,
            items: vec![(36, 1, 64)],
        };
        save_player(&dir, uuid, &data).unwrap();
        let loaded = load_player(&dir, uuid).unwrap();
        assert_eq!(loaded.health, 18.0);
        assert_eq!(loaded.items, vec![(36, 1, 64)]);
        std::fs::remove_dir_all(&dir).ok();
    }
}
