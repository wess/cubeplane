//! Server configuration, loaded from `cubeplane.toml` with sane defaults.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Top-level configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub server: ServerConfig,
    pub world: WorldConfig,
    pub mods: ModsConfig,
    pub control: ControlConfig,
    pub ai: AiConfig,
}

/// Configuration for the experimental LLM-backed villager feature.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AiConfig {
    /// Master toggle. When off, villagers fall back to ordinary trading.
    pub enabled: bool,
    /// "ollama", "openai" or "claude".
    pub provider: String,
    /// Model name (e.g. "llama3.2", "gpt-4o-mini", "claude-sonnet-4-6").
    pub model: String,
    /// API key / token (not needed for local Ollama).
    pub api_key: String,
    /// Override base URL; empty uses the provider default.
    pub base_url: String,
    /// Cap on reply length.
    pub max_tokens: u32,
    /// How many prior turns to keep as context.
    pub history_limit: usize,
    pub temperature: f32,
}

impl Default for AiConfig {
    fn default() -> Self {
        AiConfig {
            enabled: false,
            provider: "ollama".into(),
            model: "llama3.2".into(),
            api_key: String::new(),
            base_url: String::new(),
            max_tokens: 200,
            history_limit: 6,
            temperature: 0.8,
        }
    }
}

impl AiConfig {
    /// The base URL to use, applying the provider default when unset.
    pub fn effective_base_url(&self) -> String {
        if !self.base_url.is_empty() {
            return self.base_url.trim_end_matches('/').to_string();
        }
        match self.provider.as_str() {
            "openai" => "https://api.openai.com".into(),
            "claude" => "https://api.anthropic.com".into(),
            _ => "http://localhost:11434".into(),
        }
    }
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
    /// Operator player names allowed to run cheat commands. Empty = everyone
    /// is an operator (convenient for local/demo use).
    pub ops: Vec<String>,
    /// Enable protocol encryption and Mojang authentication. Requires outbound
    /// access to sessionserver.mojang.com; falls back to the client-supplied
    /// name if that lookup is unavailable.
    pub online_mode: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WorldConfig {
    /// "terrain" or "flat".
    pub generator: String,
    pub seed: u64,
    /// Persist block edits and player data to disk.
    pub save: bool,
    /// Directory for saved world/player data.
    pub save_dir: String,
    /// Persistence format: "delta" (edits over the generator) or "region"
    /// (full chunk columns, Anvil-style).
    pub format: String,
    /// Keep a player's items when they die.
    pub keep_inventory: bool,
    /// Spawn hostile mobs (otherwise only passive animals appear).
    pub spawn_hostiles: bool,
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
            ops: Vec::new(),
            online_mode: false,
        }
    }
}

impl Default for WorldConfig {
    fn default() -> Self {
        WorldConfig {
            generator: "terrain".into(),
            seed: 0x5EED,
            save: true,
            save_dir: "world_data".into(),
            format: "delta".into(),
            keep_inventory: false,
            spawn_hostiles: true,
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
