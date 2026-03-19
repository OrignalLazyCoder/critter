use std::{
    process::Command,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use super::snapshot::OsSnapshot;

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
    let mut front_owner: Option<String> = None;
    let mut front_title: Option<String> = None;
    let mut front_pid: Option<u32> = None;
    if let Some((window_count, geom, space_id, owner, title, pid)) = active_window_info() {
        snapshot.active_window_count = window_count;
        snapshot.active_window_geom_sig = geom;
        snapshot.active_space_id = space_id;
        front_owner = owner;
        front_title = title;
        front_pid = pid;
    }

    if let Some(app) = run_osascript(
        "tell application \"System Events\" to get name of first process whose frontmost is true",
    ) {
        snapshot.active_app = app.trim().to_string();
    }

    if let Some(title) = run_osascript(
        "tell application \"System Events\" to tell (first process whose frontmost is true) to try\n\
         return value of attribute \"AXTitle\" of front window\n\
         on error\n\
         return \"\"\n\
         end try",
    ) {
        snapshot.active_title = title.trim().to_string();
    }

    snapshot.active_app_pid = run_osascript(
        "tell application \"System Events\" to get unix id of first process whose frontmost is true",
    )
    .and_then(|v| v.trim().parse::<u32>().ok());

    if let Some(owner) = front_owner
        && !owner.trim().is_empty()
        && !owner.eq_ignore_ascii_case("Window Server")
    {
        let from_script = snapshot.active_app.trim();
        if from_script.is_empty()
            || from_script.eq_ignore_ascii_case("Terminal")
            || !from_script.eq_ignore_ascii_case(&owner)
        {
            snapshot.active_app = owner;
        }
    }
    if snapshot.active_title.trim().is_empty()
        && let Some(title) = front_title
        && !title.trim().is_empty()
    {
        snapshot.active_title = title;
    }
    if snapshot.active_app_pid.is_none() {
        snapshot.active_app_pid = front_pid;
    }

    snapshot.running_apps_sig = run_shell("ps -Ao comm= | sort -u | shasum | awk '{print $1}'")
        .map(|s| s.trim().to_string());

    if let Some(lock_state) = run_shell(
        "python3 - <<'PY'\n\
from Quartz import CGSessionCopyCurrentDictionary\n\
d = CGSessionCopyCurrentDictionary() or {}\n\
print('1' if d.get('CGSSessionScreenIsLocked', 0) else '0')\n\
PY",
    ) {
        snapshot.screen_locked = lock_state.trim() == "1";
    }

    if let Some(state) = run_osascript(
        "tell application \"Music\" to if it is running then get player state as text",
    ) {
        snapshot.media_playing = state.trim().eq_ignore_ascii_case("playing");
    } else {
        snapshot.media_playing = false;
    }

    snapshot.dark_mode = run_cmd("defaults", &["read", "-g", "AppleInterfaceStyle"])
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("dark"));

    if let Some((c, sig)) = cached_display_info() {
        snapshot.display_count = c;
        snapshot.display_resolution_sig = sig;
    }
    if let Some((brightness_pct, true_tone_sig)) = cached_display_dynamic_info() {
        snapshot.screen_brightness_pct = brightness_pct;
        snapshot.true_tone_sig = true_tone_sig;
    }

    snapshot.screensaver_active =
        run_shell("pgrep -x ScreenSaverEngine >/dev/null && echo 1 || echo 0")
            .is_some_and(|v| v.trim() == "1");
    snapshot.night_shift_enabled = detect_night_shift_enabled();
    snapshot.dock_autohide = run_cmd("defaults", &["read", "com.apple.dock", "autohide"])
        .is_some_and(|v| v.trim() == "1");
    snapshot.dnd_enabled = detect_dnd_enabled();

    if let Some((count, top_name, top_cpu)) = process_snapshot() {
        snapshot.process_count = count;
        snapshot.top_process = top_name;
        snapshot.top_process_cpu_pct = top_cpu;
    }

    snapshot.spotlight_opened = snapshot.active_app.eq_ignore_ascii_case("spotlight")
        || snapshot
            .active_title
            .to_ascii_lowercase()
            .contains("spotlight");
    snapshot.mission_control_active = snapshot
        .active_app
        .to_ascii_lowercase()
        .contains("mission control");
    snapshot.notification_center_visible = snapshot
        .active_app
        .to_ascii_lowercase()
        .contains("notification center");
    snapshot.app_unresponsive = snapshot
        .active_app_pid
        .and_then(process_state)
        .is_some_and(|state| state.contains('D'));
}

#[cfg(target_os = "macos")]
fn process_snapshot() -> Option<(u32, Option<String>, f32)> {
    let count_raw = run_shell("ps -A -o pid= | wc -l")?;
    let count = count_raw.trim().parse::<u32>().ok()?;

    let top_raw = run_shell("ps -Ao pcpu,comm -r | sed -n '2p'")?;
    let mut parts = top_raw.split_whitespace();
    let top_cpu = parts
        .next()
        .and_then(|s| s.parse::<f32>().ok())
        .unwrap_or(0.0);
    let top_name = parts.next().map(|s| s.to_string());

    Some((count, top_name, top_cpu))
}

#[cfg(target_os = "macos")]
fn cached_display_info() -> Option<(u16, Option<String>)> {
    static CACHE: OnceLock<Mutex<Option<(Instant, u16, Option<String>)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    let mut guard = cache.lock().ok()?;

    if let Some((ts, count, sig)) = &*guard
        && ts.elapsed() < Duration::from_secs(30)
    {
        return Some((*count, sig.clone()));
    }

    let raw = run_cmd("system_profiler", &["SPDisplaysDataType"])?;
    let resolutions: Vec<String> = raw
        .lines()
        .filter_map(|l| {
            let t = l.trim();
            if t.starts_with("Resolution:") {
                Some(t.to_string())
            } else {
                None
            }
        })
        .collect();
    let count = resolutions.len() as u16;
    if count > 0 {
        let sig = Some(resolutions.join(" | "));
        *guard = Some((Instant::now(), count, sig.clone()));
        return Some((count, sig));
    }
    None
}

#[cfg(target_os = "macos")]
fn cached_display_dynamic_info() -> Option<(Option<f32>, Option<String>)> {
    static CACHE: OnceLock<Mutex<Option<(Instant, Option<f32>, Option<String>)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    let mut guard = cache.lock().ok()?;

    if let Some((ts, brightness, true_tone_sig)) = &*guard
        && ts.elapsed() < Duration::from_secs(3)
    {
        return Some((*brightness, true_tone_sig.clone()));
    }

    let mut brightness_pct = None;
    let mut true_tone_sig = None;

    if let Some(raw) = run_cmd("ioreg", &["-lw0", "-r", "-c", "AppleARMBacklight"]) {
        brightness_pct = parse_brightness_pct_from_ioreg(&raw);
        true_tone_sig = parse_true_tone_signal_from_ioreg(&raw);
    }

    if true_tone_sig.is_none() {
        true_tone_sig = parse_true_tone_signal_from_displays_json();
    }

    *guard = Some((Instant::now(), brightness_pct, true_tone_sig.clone()));
    Some((brightness_pct, true_tone_sig))
}

#[cfg(target_os = "macos")]
fn parse_brightness_pct_from_ioreg(raw: &str) -> Option<f32> {
    let mut best: Option<f32> = None;
    for chunk in raw.split("\"brightness\"={").skip(1) {
        let Some(max_u32) = parse_ioreg_int_after(chunk, "\"max\"=") else {
            continue;
        };
        let Some(value_u32) = parse_ioreg_int_after(chunk, "\"value\"=") else {
            continue;
        };
        let max = max_u32 as f32;
        let value = value_u32 as f32;
        if max <= 0.0 {
            continue;
        }
        let pct = ((value / max) * 100.0).clamp(0.0, 100.0);
        best = Some(best.map_or(pct, |prev| prev.max(pct)));
    }
    best
}

#[cfg(target_os = "macos")]
fn parse_true_tone_signal_from_ioreg(raw: &str) -> Option<String> {
    let a = parse_ioreg_hex_blob(raw, "\"truetone-shift-a\"")?;
    let b = parse_ioreg_hex_blob(raw, "\"truetone-shift-b\"")?;
    Some(format!("a={a};b={b}"))
}

#[cfg(target_os = "macos")]
fn parse_true_tone_signal_from_displays_json() -> Option<String> {
    let raw = run_cmd("system_profiler", &["SPDisplaysDataType", "-json"])?;
    for line in raw.lines() {
        let t = line.trim();
        if !t.to_ascii_lowercase().contains("truetone") {
            continue;
        }
        let compact = t.trim_end_matches(',').replace('\"', "");
        if compact.is_empty() {
            continue;
        }
        return Some(compact);
    }
    None
}

#[cfg(target_os = "macos")]
fn parse_ioreg_int_after(s: &str, marker: &str) -> Option<u32> {
    let (_, rest) = s.split_once(marker)?;
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse::<u32>().ok()
}

#[cfg(target_os = "macos")]
fn parse_ioreg_hex_blob(s: &str, key: &str) -> Option<String> {
    let (_, rest) = s.split_once(key)?;
    let start = rest.find('<')?;
    let end = rest[start + 1..].find('>')?;
    let blob = &rest[start + 1..start + 1 + end];
    let compact: String = blob.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if compact.is_empty() {
        None
    } else {
        Some(compact.to_ascii_lowercase())
    }
}

#[cfg(target_os = "macos")]
fn detect_night_shift_enabled() -> bool {
    let candidates = [
        (
            "defaults",
            vec!["-currentHost", "read", "-g", "CBBlueLightReductionStatus"],
        ),
        (
            "defaults",
            vec![
                "-currentHost",
                "read",
                "com.apple.CoreBrightness",
                "CBBlueLightReductionStatus",
            ],
        ),
    ];

    for (cmd, args) in candidates {
        if let Some(raw) = run_cmd(cmd, &args) {
            let lc = raw.to_ascii_lowercase();
            if lc.contains("bluelightreductionenabled = 1")
                || lc.contains("bluelightreductionenabled = true")
                || lc.contains("enabled = 1")
                || lc.contains("enabled = true")
            {
                return true;
            }
            if lc.contains("bluelightreductionenabled = 0")
                || lc.contains("bluelightreductionenabled = false")
                || lc.contains("enabled = 0")
                || lc.contains("enabled = false")
            {
                return false;
            }
        }
    }

    false
}

#[cfg(target_os = "macos")]
fn detect_dnd_enabled() -> bool {
    let candidates = [
        (
            "defaults",
            vec![
                "-currentHost",
                "read",
                "com.apple.notificationcenterui",
                "doNotDisturb",
            ],
        ),
        (
            "defaults",
            vec![
                "-currentHost",
                "read",
                "com.apple.controlcenter",
                "FocusModes",
            ],
        ),
    ];
    for (cmd, args) in candidates {
        if let Some(raw) = run_cmd(cmd, &args) {
            let t = raw.trim().to_ascii_lowercase();
            if t == "1" || t == "true" || t.contains("= 1") || t.contains("= true") {
                return true;
            }
        }
    }
    false
}

#[cfg(target_os = "macos")]
type ActiveWindowInfo = (
    u32,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<u32>,
);

fn active_window_info() -> Option<ActiveWindowInfo> {
    let script = r#"python3 - <<'PY'
from Quartz import CGWindowListCopyWindowInfo, kCGWindowListOptionOnScreenOnly, kCGNullWindowID
wins = CGWindowListCopyWindowInfo(kCGWindowListOptionOnScreenOnly, kCGNullWindowID) or []
front = [w for w in wins if int(w.get('kCGWindowLayer', 1)) == 0 and int(w.get('kCGWindowAlpha', 1)) > 0]
count = len(front)
geom = ''
space = ''
owner = ''
title = ''
pid = ''
if count > 0:
    b = front[0].get('kCGWindowBounds', {})
    geom = f\"{int(b.get('X',0))},{int(b.get('Y',0))},{int(b.get('Width',0))}x{int(b.get('Height',0))}\"
    ws = front[0].get('kCGWindowWorkspace')
    if ws is not None:
        space = str(ws)
    owner = str(front[0].get('kCGWindowOwnerName', '') or '')
    title = str(front[0].get('kCGWindowName', '') or '')
    p = front[0].get('kCGWindowOwnerPID')
    if p is not None:
        pid = str(int(p))
print(count)
print(geom)
print(space)
print(owner)
print(title)
print(pid)
PY"#;
    let raw = run_shell(script)?;
    let mut lines = raw.lines();
    let count = lines.next()?.trim().parse::<u32>().ok()?;
    let geom = lines
        .next()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let space = lines
        .next()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let owner = lines
        .next()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let title = lines
        .next()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let pid = lines.next().and_then(|s| s.trim().parse::<u32>().ok());
    Some((count, geom, space, owner, title, pid))
}

#[cfg(target_os = "macos")]
fn process_state(pid: u32) -> Option<String> {
    let raw = run_cmd("ps", &["-o", "stat=", "-p", &pid.to_string()])?;
    Some(raw.trim().to_string())
}

#[cfg(target_os = "macos")]
fn run_osascript(script: &str) -> Option<String> {
    let out = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
}

#[cfg(target_os = "macos")]
fn run_shell(cmd: &str) -> Option<String> {
    let out = Command::new("sh").arg("-c").arg(cmd).output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
}

#[cfg(target_os = "macos")]
fn run_cmd(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
}
