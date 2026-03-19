use std::{fs, path::PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct CritterConfig {
    pub startup: StartupConfig,
    pub chat_persistence: ChatPersistenceConfig,
    pub network: NetworkConfig,
    pub gossip: GossipConfig,
    pub ui: UiConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StartupConfig {
    pub warn_low_color: bool,
}

impl Default for StartupConfig {
    fn default() -> Self {
        Self {
            warn_low_color: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatPersistenceConfig {
    pub enabled: bool,
    pub path: String,
    pub max_messages: usize,
    pub load_recent_count: usize,
}

impl Default for ChatPersistenceConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            path: "chat.sqlite3".to_string(),
            max_messages: 5_000,
            load_recent_count: 220,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NetworkConfig {
    pub enable_mdns: bool,
    pub enable_direct_nodeid_connect: bool,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            enable_mdns: true,
            enable_direct_nodeid_connect: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GossipConfig {
    pub spontaneous_enabled: bool,
    pub spontaneous_min_interval_secs: u64,
    pub spontaneous_max_interval_secs: u64,
    pub spontaneous_topic: String,
    pub spontaneous_content: String,
    pub allow_jokes: bool,
    pub allow_random: bool,
    pub peer_enabled: bool,
    pub peer_cooldown_secs: u64,
    pub peer_turn_spacing_secs: u64,
    pub peer_max_turns: u8,
}

impl Default for GossipConfig {
    fn default() -> Self {
        Self {
            spontaneous_enabled: true,
            spontaneous_min_interval_secs: 180,
            spontaneous_max_interval_secs: 600,
            spontaneous_topic: "mixed".to_string(),
            spontaneous_content: String::new(),
            allow_jokes: true,
            allow_random: true,
            peer_enabled: true,
            peer_cooldown_secs: 300,
            peer_turn_spacing_secs: 8,
            peer_max_turns: 4,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    pub show_debug_pane: bool,
    pub tracking_mode: String,
    pub pet_reply_frequency: String,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            show_debug_pane: false,
            tracking_mode: "essentials".to_string(),
            pet_reply_frequency: "medium".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ActivityConfig {
    pub contexts: ActivityContexts,
    pub thresholds: Thresholds,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ActivityContexts {
    pub deep_coding: StatRates,
    pub browsing: StatRates,
    pub video_call: StatRates,
    pub watching_video: StatRates,
    pub compiling: StatRates,
    pub designing: StatRates,
    pub music_coding: StatRates,
    pub idle: StatRates,
    pub rest: StatRates,
    pub late_night: StatRates,
    pub weekend: StatRates,
}

impl Default for ActivityContexts {
    fn default() -> Self {
        Self {
            deep_coding: StatRates::new(-1.4, -1.6, -0.5, 2.0),
            browsing: StatRates::new(-0.8, -0.6, 0.0, -0.6),
            video_call: StatRates::new(-1.0, -1.4, 2.5, -1.0),
            watching_video: StatRates::new(-0.5, 0.4, -0.3, -0.8),
            compiling: StatRates::new(-0.6, -0.8, -0.2, 0.5),
            designing: StatRates::new(-1.0, -1.0, -0.3, 1.2),
            music_coding: StatRates::new(-1.2, -1.2, 0.4, 1.8),
            idle: StatRates::new(-0.5, 1.0, -0.8, -1.0),
            rest: StatRates::new(-0.3, 2.2, -0.2, 0.4),
            late_night: StatRates::new(-1.8, -2.0, -1.0, 0.8),
            weekend: StatRates::new(-0.3, 1.5, 0.5, 0.0),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct Thresholds {
    pub low: f32,
    pub high: f32,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            low: 25.0,
            high: 40.0,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct StatRates {
    pub hunger: f32,
    pub energy: f32,
    pub social: f32,
    pub focus: f32,
}

impl StatRates {
    const fn new(hunger: f32, energy: f32, social: f32, focus: f32) -> Self {
        Self {
            hunger,
            energy,
            social,
            focus,
        }
    }
}

impl Default for StatRates {
    fn default() -> Self {
        Self::new(0.0, 0.0, 0.0, 0.0)
    }
}

pub fn load_or_create_activity_config() -> Result<ActivityConfig, String> {
    let cfg_dir = config_dir()?;
    let cfg_path = cfg_dir.join("activity.toml");
    load_or_create_toml(cfg_path)
}

pub fn load_or_create_critter_config() -> Result<CritterConfig, String> {
    let cfg_dir = config_dir()?;
    let cfg_path = cfg_dir.join("critter.toml");
    load_or_create_toml(cfg_path)
}

pub fn save_critter_config(cfg: &CritterConfig) -> Result<(), String> {
    let cfg_dir = config_dir()?;
    let cfg_path = cfg_dir.join("critter.toml");
    save_toml(cfg_path, cfg)
}

fn config_dir() -> Result<PathBuf, String> {
    crate::system::paths::config_dir()
}

fn load_or_create_toml<T>(cfg_path: PathBuf) -> Result<T, String>
where
    T: Default + Serialize + for<'de> Deserialize<'de>,
{
    if !cfg_path.exists() {
        let default_cfg = T::default();
        let toml = toml::to_string_pretty(&default_cfg).map_err(|e| {
            format!(
                "failed to encode default config {}: {e}",
                cfg_path.display()
            )
        })?;
        fs::write(&cfg_path, toml)
            .map_err(|e| format!("failed to write {}: {e}", cfg_path.display()))?;
    }

    let raw = fs::read_to_string(&cfg_path)
        .map_err(|e| format!("failed to read {}: {e}", cfg_path.display()))?;
    toml::from_str(&raw).map_err(|e| format!("invalid {}: {e}", cfg_path.display()))
}

fn save_toml<T>(cfg_path: PathBuf, cfg: &T) -> Result<(), String>
where
    T: Serialize,
{
    let toml = toml::to_string_pretty(cfg)
        .map_err(|e| format!("failed to encode config {}: {e}", cfg_path.display()))?;
    fs::write(&cfg_path, toml).map_err(|e| format!("failed to write {}: {e}", cfg_path.display()))
}
