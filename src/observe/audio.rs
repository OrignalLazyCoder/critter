use std::{
    process::Command,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use super::snapshot::OsSnapshot;

#[derive(Debug, Clone)]
struct AudioDeviceSnapshot {
    default_output_name: Option<String>,
    default_output_transport: Option<String>,
}

static AUDIO_DEVICE_CACHE: OnceLock<Mutex<Option<(Instant, AudioDeviceSnapshot)>>> =
    OnceLock::new();

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
    if let Some((out_vol, in_vol, alert_vol, muted)) = read_volume_settings() {
        snapshot.output_volume_pct = Some(out_vol);
        snapshot.input_volume_pct = Some(in_vol);
        snapshot.alert_volume_pct = Some(alert_vol);
        snapshot.output_muted = muted;
    }

    if let Some((playing, app, track)) = read_now_playing() {
        snapshot.media_playing = playing;
        snapshot.now_playing_app = app;
        snapshot.now_playing_track = track;
    }

    if let Some(dev) = cached_audio_devices() {
        snapshot.audio_output_device = dev.default_output_name.clone();
        snapshot.audio_output_transport = dev.default_output_transport.clone();
        let output_name = dev
            .default_output_name
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let output_transport = dev
            .default_output_transport
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase();
        snapshot.headphones_connected =
            looks_like_headphones_output(&output_name, &output_transport);
        snapshot.airplay_active =
            output_name.contains("airplay") || output_transport.contains("airplay");
    }

    snapshot.system_alert_sound_active =
        run_shell("pgrep -x afplay >/dev/null && echo 1 || echo 0")
            .is_some_and(|v| v.trim() == "1");

    if let Some(raw) = run_cmd("ioreg", &["-lw0", "-r", "-c", "AppleARMIISDevice"]) {
        snapshot.mic_active = parse_mic_active_from_ioreg(&raw);
    }
}

#[cfg(target_os = "macos")]
fn read_volume_settings() -> Option<(f32, f32, f32, bool)> {
    let raw = run_cmd("osascript", &["-e", "get volume settings"])?;
    let mut out_vol = None;
    let mut in_vol = None;
    let mut alert_vol = None;
    let mut muted = None;

    for part in raw.split(',') {
        let p = part.trim();
        if let Some(v) = p.strip_prefix("output volume:") {
            out_vol = v.trim().parse::<f32>().ok();
        } else if let Some(v) = p.strip_prefix("input volume:") {
            in_vol = v.trim().parse::<f32>().ok();
        } else if let Some(v) = p.strip_prefix("alert volume:") {
            alert_vol = v.trim().parse::<f32>().ok();
        } else if let Some(v) = p.strip_prefix("output muted:") {
            muted = Some(v.trim().eq_ignore_ascii_case("true"));
        }
    }

    Some((
        out_vol?.clamp(0.0, 100.0),
        in_vol?.clamp(0.0, 100.0),
        alert_vol?.clamp(0.0, 100.0),
        muted.unwrap_or(false),
    ))
}

#[cfg(target_os = "macos")]
fn read_now_playing() -> Option<(bool, Option<String>, Option<String>)> {
    let script = r#"set outputText to ""
set candidateApps to {"Music", "Spotify", "TV", "VLC", "QuickTime Player"}
repeat with appName in candidateApps
    try
        tell application appName
            if it is running then
                set stateText to ""
                set trackText to ""
                try
                    set stateText to (player state as text)
                end try
                try
                    set trackText to (name of current track as text)
                end try
                set outputText to outputText & appName & "|" & stateText & "|" & trackText & linefeed
            end if
        end tell
    end try
end repeat
return outputText"#;

    let raw = run_cmd("osascript", &["-e", script])?;
    let mut best_app: Option<String> = None;
    let mut best_track: Option<String> = None;
    let mut any_playing = false;

    for line in raw.lines() {
        let mut parts = line.splitn(3, '|');
        let app = parts.next().unwrap_or_default().trim();
        let state = parts.next().unwrap_or_default().trim().to_ascii_lowercase();
        let track = parts.next().unwrap_or_default().trim();
        if app.is_empty() {
            continue;
        }
        let app_s = app.to_string();
        let track_s = if track.is_empty() {
            None
        } else {
            Some(track.to_string())
        };

        if state == "playing" {
            any_playing = true;
            best_app = Some(app_s);
            best_track = track_s;
            break;
        }
        if best_app.is_none() {
            best_app = Some(app_s);
            best_track = track_s;
        }
    }

    Some((any_playing, best_app, best_track))
}

#[cfg(target_os = "macos")]
fn cached_audio_devices() -> Option<AudioDeviceSnapshot> {
    let cache = AUDIO_DEVICE_CACHE.get_or_init(|| Mutex::new(None));
    let mut guard = cache.lock().ok()?;

    if let Some((ts, snapshot)) = &*guard
        && ts.elapsed() < Duration::from_secs(10)
    {
        return Some(snapshot.clone());
    }

    let raw = run_cmd("system_profiler", &["SPAudioDataType"])?;
    let parsed = parse_audio_devices_from_profiler(&raw);
    *guard = Some((Instant::now(), parsed.clone()));
    Some(parsed)
}

#[cfg(target_os = "macos")]
fn parse_audio_devices_from_profiler(raw: &str) -> AudioDeviceSnapshot {
    #[derive(Debug, Clone, Default)]
    struct Dev {
        name: String,
        transport: Option<String>,
        is_default_output: bool,
    }

    let mut devices: Vec<Dev> = Vec::new();
    let mut current: Option<Dev> = None;

    for line in raw.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if t.ends_with(':') && t != "Audio:" && t != "Devices:" {
            if let Some(prev) = current.take() {
                devices.push(prev);
            }
            current = Some(Dev {
                name: t.trim_end_matches(':').to_string(),
                ..Dev::default()
            });
            continue;
        }
        let Some(dev) = current.as_mut() else {
            continue;
        };
        if t.starts_with("Default Output Device:") {
            dev.is_default_output = t.ends_with("Yes");
        } else if let Some(v) = t.strip_prefix("Transport:") {
            let v = v.trim();
            if !v.is_empty() {
                dev.transport = Some(v.to_string());
            }
        }
    }
    if let Some(prev) = current.take() {
        devices.push(prev);
    }

    let default_output = devices.into_iter().find(|d| d.is_default_output);
    AudioDeviceSnapshot {
        default_output_name: default_output.as_ref().map(|d| d.name.clone()),
        default_output_transport: default_output.and_then(|d| d.transport),
    }
}

#[cfg(target_os = "macos")]
fn looks_like_headphones_output(name: &str, transport: &str) -> bool {
    let markers = [
        "headphone",
        "headphones",
        "earpod",
        "airpod",
        "beats",
        "buds",
        "headset",
    ];
    markers.iter().any(|m| name.contains(m))
        || (transport.contains("bluetooth") && !name.contains("speaker"))
}

#[cfg(target_os = "macos")]
fn parse_mic_active_from_ioreg(raw: &str) -> bool {
    let mut in_input_block = false;
    let mut block_has_input_streams = false;

    for line in raw.lines() {
        let t = line.trim();
        if t.starts_with("+-o ") {
            in_input_block = t.to_ascii_lowercase().contains("input");
            block_has_input_streams = false;
            continue;
        }

        if !in_input_block {
            continue;
        }

        if t.starts_with("\"input streams\" = ") && !t.ends_with("()") {
            block_has_input_streams = true;
        }
        if block_has_input_streams && t == "\"is running\" = Yes" {
            return true;
        }
    }
    false
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
