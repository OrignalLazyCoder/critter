use std::{
    process::Command,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use super::snapshot::OsSnapshot;

#[derive(Debug, Clone, Default)]
struct SessionState {
    console_user: Option<String>,
    clamshell_closed: bool,
    timezone_sig: Option<String>,
    boot_time_epoch: Option<u64>,
    boot_session_uuid: Option<String>,
    shutdown_sig: Option<String>,
    restart_sig: Option<String>,
}

static CACHE: OnceLock<Mutex<Option<(Instant, SessionState)>>> = OnceLock::new();

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
            && ts.elapsed() < Duration::from_secs(4)
        {
            apply_state(snapshot, st.clone());
            return;
        }

        let mut st = SessionState::default();
        st.console_user = console_user();
        if let Some(raw) = run_cmd("ioreg", &["-r", "-k", "AppleClamshellState", "-d", "4"]) {
            parse_root_domain_ioreg(&raw, &mut st);
        }
        st.timezone_sig = timezone_sig();
        st.boot_time_epoch = boot_time_epoch();
        st.shutdown_sig = latest_shutdown_sig();
        st.restart_sig = latest_restart_sig();

        apply_state(snapshot, st.clone());
        *guard = Some((Instant::now(), st));
    }
}

#[cfg(target_os = "macos")]
fn apply_state(snapshot: &mut OsSnapshot, st: SessionState) {
    snapshot.console_user = st.console_user;
    snapshot.clamshell_closed = st.clamshell_closed;
    snapshot.timezone_sig = st.timezone_sig;
    snapshot.boot_time_epoch = st.boot_time_epoch;
    snapshot.boot_session_uuid = st.boot_session_uuid;
    snapshot.shutdown_sig = st.shutdown_sig;
    snapshot.restart_sig = st.restart_sig;
}

#[cfg(target_os = "macos")]
fn console_user() -> Option<String> {
    let raw = run_cmd("stat", &["-f", "%Su", "/dev/console"])?;
    let user = raw.trim();
    if user.is_empty() {
        return None;
    }
    let lower = user.to_ascii_lowercase();
    if matches!(lower.as_str(), "root" | "loginwindow" | "_mbsetupuser") {
        return None;
    }
    Some(user.to_string())
}

#[cfg(target_os = "macos")]
fn parse_root_domain_ioreg(raw: &str, st: &mut SessionState) {
    for line in raw.lines() {
        let t = line.trim();
        if let Some(v) = t.strip_prefix("\"AppleClamshellState\" = ") {
            st.clamshell_closed = v.trim().eq_ignore_ascii_case("Yes");
        } else if let Some(v) = t.strip_prefix("\"BootSessionUUID\" = ") {
            let uuid = v.trim().trim_matches('"');
            if !uuid.is_empty() {
                st.boot_session_uuid = Some(uuid.to_string());
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn timezone_sig() -> Option<String> {
    let raw = run_cmd("date", &["+%Z %z"])?;
    let sig = raw.trim();
    if sig.is_empty() {
        None
    } else {
        Some(sig.to_string())
    }
}

#[cfg(target_os = "macos")]
fn boot_time_epoch() -> Option<u64> {
    let raw = run_cmd("sysctl", &["-n", "kern.boottime"])?;
    let sec_part = raw.split("sec =").nth(1)?.split(',').next()?.trim();
    sec_part.parse::<u64>().ok()
}

#[cfg(target_os = "macos")]
fn latest_shutdown_sig() -> Option<String> {
    let raw = run_shell("last shutdown 2>/dev/null | sed -n '1p'")?;
    let s = raw.trim();
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

#[cfg(target_os = "macos")]
fn latest_restart_sig() -> Option<String> {
    let raw = run_shell("last reboot 2>/dev/null | sed -n '1p'")?;
    let s = raw.trim();
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
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
