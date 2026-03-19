use std::sync::OnceLock;
use std::{fs, path::PathBuf};

const DEFAULT_PROFILE: &str = "peer-0";
static PROFILE_OVERRIDE: OnceLock<String> = OnceLock::new();

pub fn set_profile(profile: String) {
    let _ = PROFILE_OVERRIDE.set(profile);
}

pub fn profile_name() -> String {
    if let Some(p) = PROFILE_OVERRIDE.get() {
        return p.clone();
    }
    let raw = std::env::var("CRITTER_PROFILE").unwrap_or_else(|_| DEFAULT_PROFILE.to_string());
    let clean = raw.trim();
    if clean.is_empty() {
        return DEFAULT_PROFILE.to_string();
    }
    clean
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect::<String>()
        .if_empty(DEFAULT_PROFILE)
}

pub fn data_dir() -> Result<PathBuf, String> {
    let home = std::env::var("HOME").map_err(|_| "HOME env var is not set".to_string())?;
    let dir = PathBuf::from(home)
        .join(".local/share/critter/profiles")
        .join(profile_name());
    fs::create_dir_all(&dir)
        .map_err(|e| format!("failed to create data dir {}: {e}", dir.display()))?;
    Ok(dir)
}

pub fn config_dir() -> Result<PathBuf, String> {
    let home = std::env::var("HOME").map_err(|_| "HOME env var is not set".to_string())?;
    let dir = PathBuf::from(home)
        .join(".config/critter/profiles")
        .join(profile_name());
    fs::create_dir_all(&dir)
        .map_err(|e| format!("failed to create config dir {}: {e}", dir.display()))?;
    Ok(dir)
}

trait IfEmpty {
    fn if_empty(self, fallback: &str) -> String;
}

impl IfEmpty for String {
    fn if_empty(self, fallback: &str) -> String {
        if self.is_empty() {
            fallback.to_string()
        } else {
            self
        }
    }
}
