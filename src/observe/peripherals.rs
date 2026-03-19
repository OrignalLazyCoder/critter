use std::{
    process::Command,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use super::snapshot::OsSnapshot;

#[derive(Debug, Clone, Default)]
struct PeripheralState {
    bt_power_on: bool,
    bt_connected_count: u16,
    bt_connected_sig: Option<String>,
    bt_battery_sig: Option<String>,
    airpods_connected: bool,
    airpods_in_ear: bool,
    usb_device_count: u16,
    usb_devices_sig: Option<String>,
    thunderbolt_connected_count: u16,
    thunderbolt_connected_sig: Option<String>,
    external_display_usbc: bool,
}

static CACHE: OnceLock<Mutex<Option<(Instant, PeripheralState)>>> = OnceLock::new();

pub fn poll(snapshot: &mut OsSnapshot) {
    #[cfg(target_os = "macos")]
    {
        poll_macos(snapshot);
    }

    #[cfg(target_os = "windows")]
    {
        let _ = snapshot;
    }
}

#[cfg(target_os = "macos")]
fn poll_macos(snapshot: &mut OsSnapshot) {
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    if let Ok(mut guard) = cache.lock() {
        if let Some((ts, st)) = &*guard
            && ts.elapsed() < Duration::from_secs(10)
        {
            apply_state(snapshot, st.clone());
            return;
        }

        let mut st = PeripheralState::default();
        if let Some(raw) = run_cmd("system_profiler", &["SPBluetoothDataType"]) {
            parse_bluetooth(&raw, &mut st);
        }
        if let Some(raw) = run_cmd("system_profiler", &["SPUSBDataType"]) {
            parse_usb(&raw, &mut st);
        }
        if let Some(raw) = run_cmd("system_profiler", &["SPThunderboltDataType"]) {
            parse_thunderbolt(&raw, &mut st);
        }
        if let Some(raw) = run_cmd("system_profiler", &["SPDisplaysDataType"]) {
            st.external_display_usbc = parse_external_display_usbc(&raw);
        }
        if !st.airpods_in_ear && st.airpods_connected && snapshot.media_playing {
            // Best-effort fallback when explicit in-ear metadata is unavailable.
            st.airpods_in_ear = true;
        }

        apply_state(snapshot, st.clone());
        *guard = Some((Instant::now(), st));
    }
}

#[cfg(target_os = "macos")]
fn apply_state(snapshot: &mut OsSnapshot, st: PeripheralState) {
    snapshot.bluetooth_power_on = st.bt_power_on;
    snapshot.bluetooth_connected_count = st.bt_connected_count;
    snapshot.bluetooth_connected_sig = st.bt_connected_sig;
    snapshot.bluetooth_battery_sig = st.bt_battery_sig;
    snapshot.airpods_connected = st.airpods_connected;
    snapshot.airpods_in_ear = st.airpods_in_ear;
    snapshot.usb_device_count = st.usb_device_count;
    snapshot.usb_devices_sig = st.usb_devices_sig;
    snapshot.thunderbolt_connected_count = st.thunderbolt_connected_count;
    snapshot.thunderbolt_connected_sig = st.thunderbolt_connected_sig;
    snapshot.external_display_usbc = st.external_display_usbc;
}

#[cfg(target_os = "macos")]
fn parse_bluetooth(raw: &str, st: &mut PeripheralState) {
    let mut in_connected_section = false;
    let mut current_device: Option<String> = None;
    let mut current_connected = false;
    let mut connected_devices: Vec<String> = Vec::new();
    let mut battery_levels: Vec<String> = Vec::new();
    let mut in_ear_found = false;

    for line in raw.lines() {
        let t = line.trim();
        if t == "Connected:" {
            in_connected_section = true;
            current_device = None;
            current_connected = false;
            continue;
        }
        if t == "Not Connected:" {
            in_connected_section = false;
            current_device = None;
            current_connected = false;
            continue;
        }
        if let Some(v) = t.strip_prefix("State:") {
            st.bt_power_on = v.trim().eq_ignore_ascii_case("On");
            continue;
        }

        if t.ends_with(':') {
            let heading = t.trim_end_matches(':');
            if looks_like_device_heading(heading) {
                current_device = Some(heading.to_string());
                current_connected = in_connected_section;
                if current_connected {
                    connected_devices.push(heading.to_string());
                }
                continue;
            }
        }

        if let Some(v) = t.strip_prefix("Connected:") {
            current_connected = v.trim().eq_ignore_ascii_case("Yes");
            if current_connected
                && let Some(name) = current_device.as_ref()
                && !connected_devices.iter().any(|d| d == name)
            {
                connected_devices.push(name.clone());
            }
            continue;
        }

        if !current_connected {
            continue;
        }

        if let Some(level) = parse_battery_level(t)
            && let Some(name) = current_device.as_ref()
        {
            battery_levels.push(format!("{name}:{level}"));
        }

        if t.to_ascii_lowercase().contains("in ear")
            && t.to_ascii_lowercase().contains("yes")
            && current_device
                .as_deref()
                .is_some_and(|n| is_airpods_like(n) || n.to_ascii_lowercase().contains("beats"))
        {
            in_ear_found = true;
        }
    }

    connected_devices.sort();
    connected_devices.dedup();
    st.bt_connected_count = connected_devices.len() as u16;
    if !connected_devices.is_empty() {
        st.bt_connected_sig = Some(connected_devices.join("|"));
    }

    battery_levels.sort();
    battery_levels.dedup();
    if !battery_levels.is_empty() {
        st.bt_battery_sig = Some(battery_levels.join("|"));
    }

    st.airpods_connected = connected_devices.iter().any(|d| is_airpods_like(d));
    st.airpods_in_ear = in_ear_found;
}

#[cfg(target_os = "macos")]
fn parse_usb(raw: &str, st: &mut PeripheralState) {
    let mut devices = Vec::new();
    for line in raw.lines() {
        let indent = line.chars().take_while(|c| *c == ' ').count();
        let t = line.trim();
        if !t.ends_with(':') || indent < 6 {
            continue;
        }
        let name = t.trim_end_matches(':');
        if name.starts_with("USB")
            || name.starts_with("Host Controller Driver")
            || name == "USB"
            || name == "Hub"
        {
            continue;
        }
        if name.contains("Controller") || name.contains("Bus") {
            continue;
        }
        devices.push(name.to_string());
    }
    devices.sort();
    devices.dedup();
    st.usb_device_count = devices.len() as u16;
    if !devices.is_empty() {
        st.usb_devices_sig = Some(devices.join("|"));
    }
}

#[cfg(target_os = "macos")]
fn parse_thunderbolt(raw: &str, st: &mut PeripheralState) {
    let mut statuses = Vec::new();
    for line in raw.lines() {
        let t = line.trim();
        if let Some(v) = t.strip_prefix("Status:") {
            let s = v.trim().to_string();
            if !s.to_ascii_lowercase().contains("no device connected") {
                statuses.push(s);
            }
        }
    }
    st.thunderbolt_connected_count = statuses.len() as u16;
    if !statuses.is_empty() {
        statuses.sort();
        st.thunderbolt_connected_sig = Some(statuses.join("|"));
    }
}

#[cfg(target_os = "macos")]
fn parse_external_display_usbc(raw: &str) -> bool {
    let mut has_external = false;
    let mut has_usbc_like = false;
    for line in raw.lines() {
        let t = line.trim();
        if let Some(v) = t.strip_prefix("Connection Type:") {
            let conn = v.trim().to_ascii_lowercase();
            if conn != "internal" {
                has_external = true;
            }
            if conn.contains("usb-c")
                || conn.contains("usb type-c")
                || conn.contains("thunderbolt")
                || conn.contains("displayport")
                || conn.contains("usb c")
            {
                has_usbc_like = true;
            }
        }
    }
    has_external && has_usbc_like
}

#[cfg(target_os = "macos")]
fn looks_like_device_heading(s: &str) -> bool {
    !s.is_empty()
        && !s.contains("Controller")
        && !s.contains("Bluetooth")
        && !s.contains("Vendor ID")
        && !s.contains("Product ID")
        && !s.contains("Address")
        && !s.contains("State")
        && !s.contains("Firmware")
        && !s.contains("Transport")
        && !s.contains("services")
}

#[cfg(target_os = "macos")]
fn parse_battery_level(s: &str) -> Option<String> {
    for key in ["Battery Level:", "Battery:"] {
        if let Some(v) = s.strip_prefix(key) {
            let val = v.trim();
            if !val.is_empty() {
                return Some(val.to_string());
            }
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn is_airpods_like(name: &str) -> bool {
    let l = name.to_ascii_lowercase();
    l.contains("airpods") || l.contains("air pods")
}

fn run_cmd(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
}
