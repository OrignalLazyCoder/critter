use chrono::Utc;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum MoodLevel {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoodPacket {
    pub version: u8,
    pub ts_epoch: i64,
    pub mood_level: MoodLevel,
    pub hunger_bucket: u8,
    pub energy_bucket: u8,
    pub social_bucket: u8,
    pub focus_bucket: u8,
    pub charging: bool,
    pub wifi_bucket: i8,
}

impl MoodPacket {
    pub fn from_runtime(
        hunger: u16,
        energy: u16,
        social: u16,
        focus: u16,
        charging: bool,
        wifi_rssi: Option<i32>,
    ) -> Self {
        let mood_level = MoodLevel::from_stats(hunger, energy, social, focus);
        Self {
            version: 1,
            ts_epoch: Utc::now().timestamp(),
            mood_level,
            hunger_bucket: stat_bucket(hunger),
            energy_bucket: stat_bucket(energy),
            social_bucket: stat_bucket(social),
            focus_bucket: stat_bucket(focus),
            charging,
            wifi_bucket: wifi_bucket(wifi_rssi),
        }
    }
}

impl MoodLevel {
    pub fn from_stats(hunger: u16, energy: u16, social: u16, focus: u16) -> Self {
        let avg = (hunger as u32 + energy as u32 + social as u32 + focus as u32) as f32 / 4.0;
        if avg < 35.0 {
            MoodLevel::Low
        } else if avg < 70.0 {
            MoodLevel::Medium
        } else {
            MoodLevel::High
        }
    }
}

fn stat_bucket(value: u16) -> u8 {
    match value.min(100) {
        0..=20 => 1,
        21..=40 => 2,
        41..=60 => 3,
        61..=80 => 4,
        _ => 5,
    }
}

fn wifi_bucket(rssi: Option<i32>) -> i8 {
    match rssi {
        Some(v) if v > -60 => 5,
        Some(v) if v > -68 => 4,
        Some(v) if v > -75 => 3,
        Some(v) if v > -82 => 2,
        Some(_) => 1,
        None => 0,
    }
}
