use std::{
    process::Command,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use super::snapshot::OsSnapshot;

#[derive(Debug, Clone, Default)]
struct AccessibilityState {
    accessibility_enabled: bool,
    reduce_motion_enabled: bool,
    increase_contrast_enabled: bool,
    voiceover_enabled: bool,
    focused_ui_sig: Option<String>,
    focused_ui_value_sig: Option<String>,
    selected_text_sig: Option<String>,
    scroll_position_sig: Option<String>,
}

static CACHE: OnceLock<Mutex<Option<(Instant, AccessibilityState)>>> = OnceLock::new();

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
            && ts.elapsed() < Duration::from_secs(3)
        {
            apply_state(snapshot, st.clone());
            return;
        }

        let mut st = AccessibilityState::default();
        st.reduce_motion_enabled = read_bool_default("-g", "com.apple.AccessibilityReduceMotion")
            .or_else(|| read_bool_default("com.apple.universalaccess", "reduceMotion"))
            .unwrap_or(false);
        st.increase_contrast_enabled =
            read_bool_default("-g", "com.apple.AccessibilityIncreaseContrast")
                .or_else(|| read_bool_default("com.apple.universalaccess", "IncreaseContrast"))
                .unwrap_or(false);
        st.voiceover_enabled = run_shell("pgrep -x VoiceOver >/dev/null && echo 1 || echo 0")
            .is_some_and(|v| v.trim() == "1");
        let ax_trusted = ax_process_trusted().unwrap_or(false);
        st.accessibility_enabled = st.voiceover_enabled
            || st.reduce_motion_enabled
            || st.increase_contrast_enabled
            || ax_trusted;

        if let Some((focus_sig, value_sig, selected_sig, scroll_sig)) = focused_ui_signals() {
            st.focused_ui_sig = focus_sig;
            st.focused_ui_value_sig = value_sig;
            st.selected_text_sig = selected_sig;
            st.scroll_position_sig = scroll_sig;
        }

        apply_state(snapshot, st.clone());
        *guard = Some((Instant::now(), st));
    }
}

#[cfg(target_os = "macos")]
fn apply_state(snapshot: &mut OsSnapshot, st: AccessibilityState) {
    snapshot.accessibility_enabled = st.accessibility_enabled;
    snapshot.reduce_motion_enabled = st.reduce_motion_enabled;
    snapshot.increase_contrast_enabled = st.increase_contrast_enabled;
    snapshot.voiceover_enabled = st.voiceover_enabled;
    snapshot.focused_ui_sig = st.focused_ui_sig;
    snapshot.focused_ui_value_sig = st.focused_ui_value_sig;
    snapshot.selected_text_sig = st.selected_text_sig;
    snapshot.scroll_position_sig = st.scroll_position_sig;
}

#[cfg(target_os = "macos")]
fn read_bool_default(domain: &str, key: &str) -> Option<bool> {
    let out = if domain == "-g" {
        run_cmd("defaults", &["read", "-g", key])?
    } else {
        run_cmd("defaults", &["read", domain, key])?
    };
    parse_bool_text(&out)
}

#[cfg(target_os = "macos")]
fn parse_bool_text(raw: &str) -> Option<bool> {
    let t = raw.trim().to_ascii_lowercase();
    match t.as_str() {
        "1" | "yes" | "true" => Some(true),
        "0" | "no" | "false" => Some(false),
        _ => None,
    }
}

#[cfg(target_os = "macos")]
fn ax_process_trusted() -> Option<bool> {
    let py = "from Quartz import AXIsProcessTrusted\nprint('1' if AXIsProcessTrusted() else '0')";
    let raw = run_cmd("python3", &["-c", py])?;
    Some(raw.trim() == "1")
}

#[cfg(target_os = "macos")]
fn focused_ui_signals() -> Option<(
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
)> {
    let script = r#"tell application "System Events"
try
    set p to first application process whose frontmost is true
    set appName to name of p
    set e to value of attribute "AXFocusedUIElement" of p
    set roleName to ""
    set subroleName to ""
    set titleVal to ""
    set valText to ""
    set selectedText to ""
    set scrollVal to ""
    try
        set roleName to value of attribute "AXRole" of e as text
    end try
    try
        set subroleName to value of attribute "AXSubrole" of e as text
    end try
    try
        set titleVal to value of attribute "AXTitle" of e as text
    end try
    try
        set valText to value of attribute "AXValue" of e as text
    end try
    try
        set selectedText to value of attribute "AXSelectedText" of e as text
    end try
    try
        set scrollVal to value of attribute "AXVerticalScrollBarValue" of e as text
    end try
    return appName & "|" & roleName & "|" & subroleName & "|" & titleVal & "|" & valText & "|" & selectedText & "|" & scrollVal
on error
    return ""
end try
end tell"#;
    let out = run_osascript_with_timeout(script, 2)?;
    let line = out.lines().next().unwrap_or_default().trim().to_string();
    if line.is_empty() {
        return None;
    }
    let mut parts = line.splitn(7, '|');
    let app = parts.next().unwrap_or_default().trim();
    let role = parts.next().unwrap_or_default().trim();
    let subrole = parts.next().unwrap_or_default().trim();
    let title = parts.next().unwrap_or_default().trim();
    let value = parts.next().unwrap_or_default().trim();
    let selected = parts.next().unwrap_or_default().trim();
    let scroll = parts.next().unwrap_or_default().trim();

    let focus_sig = if app.is_empty() && role.is_empty() && subrole.is_empty() && title.is_empty() {
        None
    } else {
        Some(format!("{app}|{role}|{subrole}|{title}"))
    };
    let value_sig = if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    };
    let selected_sig = if selected.is_empty() {
        None
    } else {
        let clipped: String = selected.chars().take(160).collect();
        Some(clipped)
    };
    let scroll_sig = if scroll.is_empty() {
        None
    } else {
        Some(scroll.to_string())
    };

    Some((focus_sig, value_sig, selected_sig, scroll_sig))
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
