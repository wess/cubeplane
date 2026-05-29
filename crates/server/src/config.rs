//! Server configuration, loaded from `cubeplane.toml` with sane defaults.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Top-level configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub server: ServerConfig,
    pub world: WorldConfig,
    pub mods: ModsConfig,
    pub control: ControlConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub max_players: i32,
    pub motd: String,
    pub view_distance: i32,
    /// Compression threshold in bytes; `-1` disables compression.
    pub compression_threshold: i32,
    /// "creative" or "survival".
    pub gamemode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WorldConfig {
    /// "terrain" or "flat".
    pub generator: String,
    pub seed: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ModsConfig {
    pub enabled: bool,
    pub dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ControlConfig {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    /// Optional bearer token required by the control API.
    pub token: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            server: ServerConfig::default(),
            world: WorldConfig::default(),
            mods: ModsConfig::default(),
            control: ControlConfig::default(),
        }
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            host: "0.0.0.0".into(),
            port: 25565,
            max_players: 20,
            motd: "A cubeplane server — Rust engine, JS mods".into(),
            view_distance: 8,
            compression_threshold: 256,
            gamemode: "creative".into(),
        }
    }
}

impl Default for WorldConfig {
    fn default() -> Self {
        WorldConfig {
            generator: "terrain".into(),
            seed: 0x5EED,
        }
    }
}

impl Default for ModsConfig {
    fn default() -> Self {
        ModsConfig {
            enabled: true,
            dir: "mods".into(),
        }
    }
}

impl Default for ControlConfig {
    fn default() -> Self {
        ControlConfig {
            enabled: true,
            host: "127.0.0.1".into(),
            port: 8080,
            token: None,
        }
    }
}

impl Config {
    /// Load configuration from a TOML file, falling back to defaults if the
    /// file is missing. Returns an error only for malformed TOML.
    pub fn load(path: impl AsRef<Path>) -> anyhow::Result<Config> {
        let path = path.as_ref();
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(toml::from_str(&text)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// The numeric gamemode id (0 survival, 1 creative).
    pub fn gamemode_id(&self) -> u8 {
        match self.server.gamemode.as_str() {
            "creative" => 1,
            "adventure" => 2,
            "spectator" => 3,
            _ => 0,
        }
    }

    /// Whether players spawn in creative mode.
    pub fn is_creative(&self) -> bool {
        self.gamemode_id() == 1
    }
}
