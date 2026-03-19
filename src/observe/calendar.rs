use std::{
    process::Command,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use super::snapshot::OsSnapshot;

#[derive(Debug, Clone, Default)]
struct CalendarState {
    active_sig: Option<String>,
    active_count: u16,
    max_duration_mins: u16,
    back_to_back: bool,
}

static CACHE: OnceLock<Mutex<Option<(Instant, CalendarState)>>> = OnceLock::new();

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
            && ts.elapsed() < Duration::from_secs(8)
        {
            apply_state(snapshot, st.clone());
            return;
        }

        let st = query_calendar_state().unwrap_or_default();
        apply_state(snapshot, st.clone());
        *guard = Some((Instant::now(), st));
    }
}

#[cfg(target_os = "macos")]
fn apply_state(snapshot: &mut OsSnapshot, st: CalendarState) {
    snapshot.calendar_active_sig = st.active_sig;
    snapshot.calendar_active_count = st.active_count;
    snapshot.calendar_max_duration_mins = st.max_duration_mins;
    snapshot.calendar_back_to_back = st.back_to_back;
}

#[cfg(target_os = "macos")]
fn query_calendar_state() -> Option<CalendarState> {
    let script = r#"tell application "Calendar"
set nowDate to current date
set activeEvents to {}
set upcomingEvents to {}
repeat with c in calendars
    set activeEvents to activeEvents & (every event of c whose start date ≤ nowDate and end date ≥ nowDate)
    set upcomingEvents to upcomingEvents & (every event of c whose start date > nowDate and start date < (nowDate + (4 * hours)))
end repeat
set activeCount to count of activeEvents
set activeSig to ""
set maxDur to 0
repeat with e in activeEvents
    try
        set evDur to ((end date of e) - (start date of e)) / 60
        if evDur > maxDur then set maxDur to evDur
    end try
    try
        set activeSig to activeSig & (uid of e as text) & ";"
    on error
        set activeSig to activeSig & (summary of e as text) & ";"
    end try
end repeat
set backToBack to false
repeat with a in activeEvents
    repeat with u in upcomingEvents
        try
            set gapMin to ((start date of u) - (end date of a)) / 60
            if gapMin ≥ 0 and gapMin ≤ 15 then
                set backToBack to true
                exit repeat
            end if
        end try
    end repeat
    if backToBack then exit repeat
end repeat
return (activeCount as text) & "|" & (maxDur as text) & "|" & (backToBack as text) & "|" & activeSig
end tell"#;

    let out = run_osascript_with_timeout(script, 2)?;
    let line = out.lines().next().unwrap_or_default().trim();
    if line.is_empty() {
        return Some(CalendarState::default());
    }
    let mut parts = line.splitn(4, '|');
    let count = parts
        .next()
        .and_then(|v| v.trim().parse::<u16>().ok())
        .unwrap_or(0);
    let max_dur = parts
        .next()
        .and_then(|v| {
            let n = v.trim().parse::<f32>().ok()?;
            Some(n.round() as u16)
        })
        .unwrap_or(0);
    let back_to_back = parts
        .next()
        .map(|v| v.trim().eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let sig = parts
        .next()
        .map(|v| v.trim().to_string())
        .filter(|s| !s.is_empty() && s != "\"\"");

    Some(CalendarState {
        active_sig: sig,
        active_count: count,
        max_duration_mins: max_dur,
        back_to_back,
    })
}

#[cfg(target_os = "macos")]
fn run_osascript_with_timeout(script: &str, timeout_secs: u64) -> Option<String> {
    let py = r#"import subprocess, sys
script = sys.argv[1]
timeout = float(sys.argv[2])
try:
    p = subprocess.run(["osascript", "-e", script], capture_output=True, text=True, timeout=timeout)
    if p.returncode != 0:
        sys.exit(1)
    sys.stdout.write(p.stdout)
except Exception:
    sys.exit(1)
"#;
    let out = Command::new("python3")
        .arg("-c")
        .arg(py)
        .arg(script)
        .arg(timeout_secs.to_string())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
}
