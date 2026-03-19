use chrono::{Datelike, Local, Timelike, Weekday};

use super::snapshot::OsSnapshot;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityContext {
    Unknown,
    DeepCoding,
    Browsing,
    VideoCall,
    WatchingVideo,
    Compiling,
    Designing,
    MusicCoding,
    Idle,
    Rest,
    LateNight,
    Weekend,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct StatDelta {
    pub hunger_per_min: f32,
    pub energy_per_min: f32,
    pub social_per_min: f32,
    pub focus_per_min: f32,
}

pub fn classify(snapshot: &OsSnapshot) -> ActivityContext {
    if is_weekend() && snapshot.idle_secs > 120 {
        return ActivityContext::Weekend;
    }
    if is_late_night() && snapshot.idle_secs < 60 {
        return ActivityContext::LateNight;
    }
    if snapshot.idle_secs > 900 {
        return ActivityContext::Rest;
    }
    if snapshot.idle_secs > 120 {
        return ActivityContext::Idle;
    }

    let app = snapshot.active_app.to_ascii_lowercase();
    let title = snapshot.active_title.to_ascii_lowercase();

    if is_video_call_app(&app) || (snapshot.net_tx_kbps > 900 && app.contains("zoom")) {
        return ActivityContext::VideoCall;
    }
    if looks_like_compiling(&app, &title) {
        return ActivityContext::Compiling;
    }
    if looks_like_designing(&app) {
        return ActivityContext::Designing;
    }
    if looks_like_browser(&app) {
        if snapshot.net_rx_kbps > 1200 && snapshot.key_wpm < 8.0 {
            return ActivityContext::WatchingVideo;
        }
        return ActivityContext::Browsing;
    }
    if looks_like_ide_or_terminal(&app) {
        if snapshot.media_playing && snapshot.key_wpm >= 20.0 {
            return ActivityContext::MusicCoding;
        }
        if snapshot.key_wpm >= 20.0 {
            return ActivityContext::DeepCoding;
        }
    }

    ActivityContext::Unknown
}

pub fn deltas_for(context: ActivityContext) -> StatDelta {
    match context {
        ActivityContext::DeepCoding => StatDelta::new(-1.4, -1.6, -0.5, 2.0),
        ActivityContext::Browsing => StatDelta::new(-0.8, -0.6, 0.0, -0.6),
        ActivityContext::VideoCall => StatDelta::new(-1.0, -1.4, 2.5, -1.0),
        ActivityContext::WatchingVideo => StatDelta::new(-0.5, 0.4, -0.3, -0.8),
        ActivityContext::Compiling => StatDelta::new(-0.6, -0.8, -0.2, 0.5),
        ActivityContext::Designing => StatDelta::new(-1.0, -1.0, -0.3, 1.2),
        ActivityContext::MusicCoding => StatDelta::new(-1.2, -1.2, 0.4, 1.8),
        ActivityContext::Idle => StatDelta::new(-0.5, 1.0, -0.8, -1.0),
        ActivityContext::Rest => StatDelta::new(-0.3, 2.2, -0.2, 0.4),
        ActivityContext::LateNight => StatDelta::new(-1.8, -2.0, -1.0, 0.8),
        ActivityContext::Weekend => StatDelta::new(-0.3, 1.5, 0.5, 0.0),
        ActivityContext::Unknown => StatDelta::default(),
    }
}

impl StatDelta {
    const fn new(
        hunger_per_min: f32,
        energy_per_min: f32,
        social_per_min: f32,
        focus_per_min: f32,
    ) -> Self {
        Self {
            hunger_per_min,
            energy_per_min,
            social_per_min,
            focus_per_min,
        }
    }
}

fn is_weekend() -> bool {
    matches!(Local::now().weekday(), Weekday::Sat | Weekday::Sun)
}

fn is_late_night() -> bool {
    let hour = Local::now().hour();
    (23..=23).contains(&hour) || (0..=4).contains(&hour)
}

fn looks_like_browser(app: &str) -> bool {
    ["chrome", "safari", "firefox", "arc", "brave", "edge"]
        .iter()
        .any(|k| app.contains(k))
}

fn looks_like_ide_or_terminal(app: &str) -> bool {
    [
        "cursor", "code", "vscode", "xcode", "terminal", "iterm", "wezterm", "zed", "emacs",
    ]
    .iter()
    .any(|k| app.contains(k))
}

fn is_video_call_app(app: &str) -> bool {
    ["zoom", "meet", "teams", "webex", "slack huddle"]
        .iter()
        .any(|k| app.contains(k))
}

fn looks_like_designing(app: &str) -> bool {
    ["figma", "sketch", "canva", "photoshop"]
        .iter()
        .any(|k| app.contains(k))
}

fn looks_like_compiling(app: &str, title: &str) -> bool {
    let haystack = format!("{app} {title}");
    ["cargo", "rustc", "webpack", "clang", "gcc", "npm run build"]
        .iter()
        .any(|k| haystack.contains(k))
}
