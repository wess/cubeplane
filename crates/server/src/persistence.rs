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
