use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SharedState {
    pub user_name: String,
    pub pet_name: String,
    pub mood: String,
    pub hunger: u16,
    pub energy: u16,
    pub social: u16,
    pub focus: u16,
    pub hw: SharedHwState,
    pub messages: Vec<String>,
    pub peers: Vec<SharedPeerState>,
    pub gossip_lines: Vec<String>,
    #[serde(default)]
    pub gossip_rate_remaining_secs: u64,
    #[serde(default = "default_gossip_rate_total")]
    pub gossip_rate_total_secs: u64,
    pub ts: u64,
}

fn default_gossip_rate_total() -> u64 {
    300
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SharedHwState {
    pub wifi_rssi: Option<i32>,
    pub wifi_ssid: Option<String>,
    pub battery_pct: Option<f32>,
    pub charging: bool,
    pub cpu_temp_c: Option<f32>,
    pub cpu_pct: f32,
    pub ram_pct: f32,
    pub net_tx_kbps: u32,
    pub active_app: String,
    pub idle_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SharedPeerState {
    pub node_id: String,
    pub pet_name: String,
    pub activity: String,
    pub status: String,
}
