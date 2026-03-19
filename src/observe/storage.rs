use std::{
    process::Command,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use super::snapshot::OsSnapshot;

static DISK_IO_CACHE: OnceLock<Mutex<Option<(Instant, f32)>>> = OnceLock::new();

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
    if let Some((sig, external_count)) = mounted_volumes() {
        snapshot.mounted_volumes_sig = Some(sig);
        snapshot.external_volume_count = external_count;
    }

    snapshot.disk_io_mbps = cached_disk_io_mbps();

    if let Some(running) = time_machine_running() {
        snapshot.time_machine_running = running;
    }

    snapshot.watched_dir_sig = watched_dir_signature(".");
    snapshot.large_write_sig = detect_large_recent_write(".");

    if let Some(count) = trash_item_count() {
        snapshot.trash_item_count = count;
    }
    snapshot.downloads_sig = latest_download_signature();

    if let Some((smart_status, healthy)) = ssd_health() {
        snapshot.ssd_smart_status = Some(smart_status);
        snapshot.ssd_healthy = healthy;
    } else {
        snapshot.ssd_healthy = true;
    }
}

#[cfg(target_os = "macos")]
fn mounted_volumes() -> Option<(String, u16)> {
    let raw = run_cmd("mount", &[])?;
    let mut mounts: Vec<String> = Vec::new();
    let mut external_count: u16 = 0;
    for line in raw.lines() {
        let mount_point = line
            .split(" on ")
            .nth(1)
            .and_then(|rest| rest.split(" (").next());
        let Some(mp) = mount_point else {
            continue;
        };
        let m = mp.trim().to_string();
        if m.starts_with("/Volumes/") {
            external_count = external_count.saturating_add(1);
        }
        mounts.push(m);
    }
    mounts.sort();
    Some((mounts.join("|"), external_count))
}

#[cfg(target_os = "macos")]
fn cached_disk_io_mbps() -> Option<f32> {
    let cache = DISK_IO_CACHE.get_or_init(|| Mutex::new(None));
    let mut guard = cache.lock().ok()?;
    if let Some((ts, mbps)) = *guard
        && ts.elapsed() < Duration::from_secs(12)
    {
        return Some(mbps);
    }

    let raw = run_cmd("iostat", &["-d", "-w", "1", "-c", "2", "disk0"])?;
    let mbps = parse_iostat_mbps(&raw)?;
    *guard = Some((Instant::now(), mbps));
    Some(mbps)
}

#[cfg(target_os = "macos")]
fn parse_iostat_mbps(raw: &str) -> Option<f32> {
    let mut parsed: Option<f32> = None;
    for line in raw.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with("disk0") || t.starts_with("KB/t") {
            continue;
        }
        let cols: Vec<&str> = t.split_whitespace().collect();
        let Some(last) = cols.last() else {
            continue;
        };
        if let Ok(v) = last.parse::<f32>() {
            parsed = Some(v.max(0.0));
        }
    }
    parsed
}

#[cfg(target_os = "macos")]
fn time_machine_running() -> Option<bool> {
    let raw = run_cmd("tmutil", &["status"])?;
    for line in raw.lines() {
        let t = line.trim();
        if t.starts_with("Running") {
            return Some(t.contains("= 1"));
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn watched_dir_signature(path: &str) -> Option<String> {
    let cmd = format!(
        "find {} -maxdepth 2 -type f -exec stat -f '%m:%z:%N' {{}} + 2>/dev/null | LC_ALL=C sort | shasum | awk '{{print $1}}'",
        shell_escape(path)
    );
    run_shell(&cmd)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(target_os = "macos")]
fn detect_large_recent_write(path: &str) -> Option<String> {
    let now = unix_ts();
    let cmd = format!(
        "find {} -maxdepth 2 -type f -exec stat -f '%m %z %N' {{}} + 2>/dev/null | LC_ALL=C sort -nr",
        shell_escape(path)
    );
    let raw = run_shell(&cmd)?;
    for line in raw.lines() {
        let mut parts = line.splitn(3, ' ');
        let mtime = parts.next().and_then(|v| v.parse::<u64>().ok())?;
        let size = parts.next().and_then(|v| v.parse::<u64>().ok())?;
        let name = parts.next().unwrap_or_default().trim();
        if name.is_empty() {
            continue;
        }
        if size >= 25 * 1024 * 1024 && now.saturating_sub(mtime) <= 20 {
            return Some(format!("{name}:{size}:{mtime}"));
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn trash_item_count() -> Option<u32> {
    let raw = run_shell("find \"$HOME/.Trash\" -mindepth 1 -maxdepth 1 2>/dev/null | wc -l")?;
    raw.trim().parse::<u32>().ok()
}

#[cfg(target_os = "macos")]
fn latest_download_signature() -> Option<String> {
    let raw = run_shell(
        "find \"$HOME/Downloads\" -maxdepth 1 -type f -exec stat -f '%m %z %N' {} + 2>/dev/null | LC_ALL=C sort -nr",
    )?;
    for line in raw.lines() {
        let mut parts = line.splitn(3, ' ');
        let mtime = parts.next().and_then(|v| v.parse::<u64>().ok())?;
        let size = parts.next().and_then(|v| v.parse::<u64>().ok())?;
        let name = parts.next().unwrap_or_default().trim();
        if name.is_empty() {
            continue;
        }
        let lower = name.to_ascii_lowercase();
        if lower.ends_with(".download")
            || lower.ends_with(".part")
            || lower.ends_with(".crdownload")
        {
            continue;
        }
        return Some(format!("{name}:{size}:{mtime}"));
    }
    None
}

#[cfg(target_os = "macos")]
fn ssd_health() -> Option<(String, bool)> {
    let raw = run_cmd("diskutil", &["info", "/"])?;
    let mut smart = None;
    for line in raw.lines() {
        let t = line.trim();
        if let Some(v) = t.strip_prefix("SMART Status:") {
            smart = Some(v.trim().to_string());
            break;
        }
    }
    let status = smart?;
    let healthy = status.eq_ignore_ascii_case("verified");
    Some((status, healthy))
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

fn unix_ts() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn shell_escape(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}
