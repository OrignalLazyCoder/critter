use std::{
    collections::VecDeque,
    process::Command,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use super::snapshot::OsSnapshot;

#[derive(Debug, Clone)]
struct InputState {
    last_idle_secs: u64,
    last_at: Instant,
    last_key_age_s: f32,
    last_click_age_s: f32,
    last_scroll_age_s: f32,
    clipboard_hash: Option<u64>,
    clipboard_changes: VecDeque<Instant>,
    click_events: VecDeque<Instant>,
}

static INPUT_STATE: OnceLock<Mutex<Option<InputState>>> = OnceLock::new();

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
    if let Some(idle_ns) = read_idle_ns() {
        snapshot.idle_secs = idle_ns / 1_000_000_000;
    }

    if let Some((key_age, move_age, click_age, scroll_age)) = read_event_ages() {
        snapshot.key_event_age_s = key_age;
        snapshot.mouse_move_age_s = move_age;
        snapshot.mouse_click_age_s = click_age;
        snapshot.scroll_event_age_s = scroll_age;
    }

    let state = INPUT_STATE.get_or_init(|| Mutex::new(None));
    if let Ok(mut guard) = state.lock() {
        let now = Instant::now();
        let mut st = guard.take().unwrap_or(InputState {
            last_idle_secs: snapshot.idle_secs,
            last_at: now,
            last_key_age_s: snapshot.key_event_age_s,
            last_click_age_s: snapshot.mouse_click_age_s,
            last_scroll_age_s: snapshot.scroll_event_age_s,
            clipboard_hash: None,
            clipboard_changes: VecDeque::new(),
            click_events: VecDeque::new(),
        });

        let mut estimated_wpm = match snapshot.key_event_age_s {
            a if a < 0.15 => 55.0,
            a if a < 0.40 => 38.0,
            a if a < 0.80 => 24.0,
            a if a < 2.00 => 12.0,
            _ => 0.0,
        };

        let dt = now.duration_since(st.last_at).as_secs_f32();
        if dt > 0.0 && snapshot.idle_secs + 1 < st.last_idle_secs {
            estimated_wpm += 8.0;
        }
        snapshot.key_wpm = estimated_wpm;

        snapshot.shortcut_burst =
            snapshot.key_wpm >= 35.0 && snapshot.key_event_age_s < 0.25 && st.last_key_age_s > 1.0;

        if snapshot.mouse_click_age_s < 0.20 && st.last_click_age_s > 0.80 {
            st.click_events.push_back(now);
        }
        while st
            .click_events
            .front()
            .is_some_and(|t| now.duration_since(*t) > Duration::from_secs(10))
        {
            let _ = st.click_events.pop_front();
        }
        snapshot.mouse_click_rate = st.click_events.len() as u32;

        snapshot.clipboard_changed = false;
        if let Some(hash) = clipboard_hash() {
            if st.clipboard_hash.is_some_and(|h| h != hash) {
                snapshot.clipboard_changed = true;
                st.clipboard_changes.push_back(now);
            }
            st.clipboard_hash = Some(hash);
        }
        while st
            .clipboard_changes
            .front()
            .is_some_and(|t| now.duration_since(*t) > Duration::from_secs(60))
        {
            let _ = st.clipboard_changes.pop_front();
        }
        snapshot.clipboard_change_rate = st.clipboard_changes.len() as u32;

        st.last_idle_secs = snapshot.idle_secs;
        st.last_at = now;
        st.last_key_age_s = snapshot.key_event_age_s;
        st.last_click_age_s = snapshot.mouse_click_age_s;
        st.last_scroll_age_s = snapshot.scroll_event_age_s;
        *guard = Some(st);
    }
}

#[cfg(target_os = "macos")]
fn read_event_ages() -> Option<(f32, f32, f32, f32)> {
    let script = r#"python3 - <<'PY'
from Quartz import (
    CGEventSourceSecondsSinceLastEventType,
    kCGEventSourceStateCombinedSessionState,
    kCGEventKeyDown,
    kCGEventMouseMoved,
    kCGEventLeftMouseDown,
    kCGEventRightMouseDown,
    kCGEventOtherMouseDown,
    kCGEventScrollWheel,
)
src = kCGEventSourceStateCombinedSessionState
key = CGEventSourceSecondsSinceLastEventType(src, kCGEventKeyDown)
move = CGEventSourceSecondsSinceLastEventType(src, kCGEventMouseMoved)
left = CGEventSourceSecondsSinceLastEventType(src, kCGEventLeftMouseDown)
right = CGEventSourceSecondsSinceLastEventType(src, kCGEventRightMouseDown)
other = CGEventSourceSecondsSinceLastEventType(src, kCGEventOtherMouseDown)
click = min(left, right, other)
scroll = CGEventSourceSecondsSinceLastEventType(src, kCGEventScrollWheel)
print(f\"{key:.6f} {move:.6f} {click:.6f} {scroll:.6f}\")
PY"#;

    let raw = run_shell(script)?;
    let mut p = raw.split_whitespace();
    let key = p.next()?.parse::<f32>().ok()?;
    let move_age = p.next()?.parse::<f32>().ok()?;
    let click = p.next()?.parse::<f32>().ok()?;
    let scroll = p.next()?.parse::<f32>().ok()?;
    Some((key, move_age, click, scroll))
}

#[cfg(target_os = "macos")]
fn clipboard_hash() -> Option<u64> {
    let raw = run_shell("pbpaste 2>/dev/null | head -c 4096 | shasum | awk '{print $1}'")?;
    let hex = raw.trim();
    let short = if hex.len() > 16 { &hex[..16] } else { hex };
    u64::from_str_radix(short, 16).ok()
}

#[cfg(target_os = "macos")]
fn read_idle_ns() -> Option<u64> {
    let raw = run_shell("ioreg -c IOHIDSystem | awk '/HIDIdleTime/ {print $NF; exit}'")?;
    let trimmed = raw.trim();
    if let Some(hex) = trimmed.strip_prefix("0x") {
        return u64::from_str_radix(hex, 16).ok();
    }
    trimmed.parse::<u64>().ok()
}

#[cfg(target_os = "macos")]
fn run_shell(cmd: &str) -> Option<String> {
    let out = Command::new("sh").arg("-c").arg(cmd).output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
}
