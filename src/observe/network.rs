use std::{
    process::Command,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use super::snapshot::OsSnapshot;

#[derive(Debug, Clone, Copy)]
struct NetworkState {
    last_in_bytes: u64,
    last_out_bytes: u64,
    last_at: Instant,
}

static NET_STATE: OnceLock<Mutex<Option<NetworkState>>> = OnceLock::new();
static WIFI_RSSI_CACHE: OnceLock<Mutex<Option<(Instant, i32)>>> = OnceLock::new();

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
    let wifi_iface = wifi_interface();

    if let Some(airport_raw) = run_cmd(
        "/System/Library/PrivateFrameworks/Apple80211.framework/Versions/Current/Resources/airport",
        &["-I"],
    ) {
        parse_airport_info(&airport_raw, snapshot);
    }

    if snapshot.wifi_ssid.is_none()
        && let Some(iface) = wifi_iface.as_deref()
        && let Some(ssid) = current_airport_network(iface)
    {
        snapshot.wifi_ssid = Some(ssid);
    }

    if snapshot.wifi_ssid.is_none() {
        if let Some(iface) = wifi_iface.as_deref()
            && let Some(raw) = run_cmd("ipconfig", &["getsummary", iface])
        {
            parse_ipconfig_wifi(&raw, snapshot);
        }
    }

    if snapshot.wifi_rssi.is_none() {
        snapshot.wifi_rssi = cached_wifi_rssi();
    }

    if let Some(iface) = default_interface().or_else(|| wifi_iface.clone()) {
        snapshot.active_interface = Some(iface.clone());
        snapshot.network_up = true;
        if let Some((in_bytes, out_bytes)) = read_iface_totals(&iface) {
            let state = NET_STATE.get_or_init(|| Mutex::new(None));
            if let Ok(mut guard) = state.lock() {
                let now = Instant::now();
                if let Some(prev) = *guard {
                    let dt = now.duration_since(prev.last_at).as_secs_f64();
                    if dt > 0.0 {
                        let in_kbps = ((in_bytes.saturating_sub(prev.last_in_bytes) as f64 * 8.0)
                            / 1000.0
                            / dt)
                            .max(0.0);
                        let out_kbps = ((out_bytes.saturating_sub(prev.last_out_bytes) as f64
                            * 8.0)
                            / 1000.0
                            / dt)
                            .max(0.0);
                        snapshot.net_rx_kbps = in_kbps.round() as u32;
                        snapshot.net_tx_kbps = out_kbps.round() as u32;
                    }
                }

                *guard = Some(NetworkState {
                    last_in_bytes: in_bytes,
                    last_out_bytes: out_bytes,
                    last_at: now,
                });
            }
        }
    }

    if let Some(raw) = run_cmd("scutil", &["--nc", "list"]) {
        snapshot.vpn_active = raw.lines().any(|line| line.contains("Connected"));
    }
}

#[cfg(target_os = "macos")]
fn parse_airport_info(raw: &str, snapshot: &mut OsSnapshot) {
    for line in raw.lines() {
        let t = line.trim();
        if let Some(v) = t.strip_prefix("SSID:") {
            let ssid = sanitize_ssid(v);
            if !ssid.is_empty() {
                snapshot.wifi_ssid = Some(ssid);
            }
        } else if let Some(v) = t.strip_prefix("agrCtlRSSI:") {
            if let Ok(rssi) = v.trim().parse::<i32>() {
                snapshot.wifi_rssi = Some(rssi);
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn parse_ipconfig_wifi(raw: &str, snapshot: &mut OsSnapshot) {
    for line in raw.lines() {
        let t = line.trim();
        if let Some(v) = t.strip_prefix("SSID :") {
            let ssid = sanitize_ssid(v);
            if !ssid.is_empty() {
                snapshot.wifi_ssid = Some(ssid);
            }
        } else if let Some(v) = t.strip_prefix("LinkStatusActive :") {
            snapshot.network_up = v.trim().eq_ignore_ascii_case("TRUE");
        }
    }
}

#[cfg(target_os = "macos")]
fn current_airport_network(iface: &str) -> Option<String> {
    let raw = run_cmd("networksetup", &["-getairportnetwork", iface])?;
    for line in raw.lines() {
        let line = line.trim();
        if line.contains("You are not associated") {
            return None;
        }
        if let Some((_, v)) = line.split_once(':') {
            let ssid = sanitize_ssid(v);
            if !ssid.is_empty() {
                return Some(ssid);
            }
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn sanitize_ssid(raw: &str) -> String {
    raw.trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim()
        .to_string()
}

#[cfg(target_os = "macos")]
fn default_interface() -> Option<String> {
    let raw = run_cmd("route", &["-n", "get", "default"])?;
    for line in raw.lines() {
        let t = line.trim();
        if let Some(v) = t.strip_prefix("interface:") {
            return Some(v.trim().to_string());
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn wifi_interface() -> Option<String> {
    let raw = run_cmd("networksetup", &["-listallhardwareports"])?;
    let mut saw_wifi_port = false;
    for line in raw.lines() {
        let t = line.trim();
        if let Some(port) = t.strip_prefix("Hardware Port:") {
            let port = port.trim().to_ascii_lowercase();
            saw_wifi_port = port.contains("wi-fi")
                || port.contains("wifi")
                || port.contains("airport")
                || port.contains("airpot");
            continue;
        }
        if saw_wifi_port {
            if let Some(dev) = t.strip_prefix("Device:") {
                return Some(dev.trim().to_string());
            }
            if t.starts_with("Hardware Port:") {
                saw_wifi_port = false;
            }
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn cached_wifi_rssi() -> Option<i32> {
    let cache = WIFI_RSSI_CACHE.get_or_init(|| Mutex::new(None));
    if let Ok(mut guard) = cache.lock() {
        if let Some((ts, rssi)) = *guard
            && ts.elapsed() < Duration::from_secs(30)
        {
            return Some(rssi);
        }

        let raw = run_cmd("system_profiler", &["SPAirPortDataType"])?;
        let mut in_current_section = false;
        for line in raw.lines() {
            let t = line.trim();
            if t == "Current Network Information:" {
                in_current_section = true;
                continue;
            }
            if in_current_section && t.starts_with("Other Local Wi-Fi Networks:") {
                break;
            }
            if in_current_section && t.starts_with("Signal / Noise:") {
                let first = t
                    .split(':')
                    .nth(1)
                    .unwrap_or_default()
                    .split('/')
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .split_whitespace()
                    .next()
                    .unwrap_or_default();
                if let Ok(v) = first.parse::<i32>() {
                    *guard = Some((Instant::now(), v));
                    return Some(v);
                }
            }
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn read_iface_totals(iface: &str) -> Option<(u64, u64)> {
    let cmd = format!(
        "netstat -bI {iface} | awk 'NR>1 {{inb+=$7; outb+=$10}} END {{printf \"%llu %llu\", inb, outb}}'"
    );
    let raw = run_shell(&cmd)?;
    let mut parts = raw.split_whitespace();
    let in_bytes = parts.next()?.parse::<u64>().ok()?;
    let out_bytes = parts.next()?.parse::<u64>().ok()?;
    Some((in_bytes, out_bytes))
}

fn run_cmd(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
}

fn run_shell(cmd: &str) -> Option<String> {
    let out = Command::new("sh").arg("-c").arg(cmd).output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
}
