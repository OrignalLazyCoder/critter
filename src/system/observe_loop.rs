use std::{
    sync::mpsc::{self, Receiver, SyncSender, TrySendError},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::observe;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackingMode {
    Essentials,
    All,
    None,
    Custom,
}

impl TrackingMode {
    pub fn as_str(self) -> &'static str {
        match self {
            TrackingMode::Essentials => "essentials",
            TrackingMode::All => "all",
            TrackingMode::None => "none",
            TrackingMode::Custom => "custom",
        }
    }

    pub fn from_str(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "all" => TrackingMode::All,
            "none" => TrackingMode::None,
            "custom" => TrackingMode::Custom,
            _ => TrackingMode::Essentials,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TrackerToggles {
    pub hardware: bool,
    pub network: bool,
    pub process: bool,
    pub input: bool,
    pub session: bool,
    pub audio: bool,
    pub storage: bool,
    pub calendar: bool,
    pub environment: bool,
    pub accessibility: bool,
    pub peripherals: bool,
}

impl Default for TrackerToggles {
    fn default() -> Self {
        Self {
            hardware: true,
            network: true,
            process: true,
            input: true,
            session: true,
            audio: true,
            storage: true,
            calendar: true,
            environment: true,
            accessibility: true,
            peripherals: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TrackerConfig {
    pub mode: TrackingMode,
    pub custom: TrackerToggles,
}

impl Default for TrackerConfig {
    fn default() -> Self {
        Self {
            mode: TrackingMode::All,
            custom: TrackerToggles::default(),
        }
    }
}

impl TrackerConfig {
    pub fn is_enabled(&self, key: &str) -> bool {
        match self.mode {
            TrackingMode::All => true,
            TrackingMode::None => false,
            TrackingMode::Essentials => matches!(
                key,
                "hardware" | "network" | "process" | "input" | "session"
            ),
            TrackingMode::Custom => self.custom_enabled(key),
        }
    }

    pub fn set_custom_enabled(&mut self, key: &str, enabled: bool) {
        match key {
            "hardware" => self.custom.hardware = enabled,
            "network" => self.custom.network = enabled,
            "process" => self.custom.process = enabled,
            "input" => self.custom.input = enabled,
            "session" => self.custom.session = enabled,
            "audio" => self.custom.audio = enabled,
            "storage" => self.custom.storage = enabled,
            "calendar" => self.custom.calendar = enabled,
            "environment" => self.custom.environment = enabled,
            "accessibility" => self.custom.accessibility = enabled,
            "peripherals" => self.custom.peripherals = enabled,
            _ => {}
        }
    }

    pub fn custom_enabled(&self, key: &str) -> bool {
        match key {
            "hardware" => self.custom.hardware,
            "network" => self.custom.network,
            "process" => self.custom.process,
            "input" => self.custom.input,
            "session" => self.custom.session,
            "audio" => self.custom.audio,
            "storage" => self.custom.storage,
            "calendar" => self.custom.calendar,
            "environment" => self.custom.environment,
            "accessibility" => self.custom.accessibility,
            "peripherals" => self.custom.peripherals,
            _ => false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ObserveControl {
    inner: Arc<Mutex<TrackerConfig>>,
}

impl ObserveControl {
    pub fn new(config: TrackerConfig) -> Self {
        Self {
            inner: Arc::new(Mutex::new(config)),
        }
    }

    pub fn set(&self, config: TrackerConfig) {
        if let Ok(mut c) = self.inner.lock() {
            *c = config;
        }
    }

    pub fn get(&self) -> TrackerConfig {
        self.inner.lock().map(|c| c.clone()).unwrap_or_default()
    }
}

pub struct ObserveHandle {
    pub rx: Receiver<observe::snapshot::OsSnapshot>,
    pub control: ObserveControl,
}

pub const TRACKER_OPTIONS: [(&str, &str); 11] = [
    ("hardware", "hardware"),
    ("network", "network"),
    ("process", "applications & windows"),
    ("input", "keyboard, mouse & trackpad"),
    ("session", "session & user state"),
    ("audio", "audio & media"),
    ("storage", "storage & filesystem"),
    ("calendar", "calendar & notifications"),
    ("environment", "location & environment"),
    ("accessibility", "accessibility & system state"),
    ("peripherals", "bluetooth & peripherals"),
];

pub(crate) fn start_observe_thread(
    observe_tick: Duration,
) -> Receiver<observe::snapshot::OsSnapshot> {
    start_observe_thread_controlled(observe_tick, TrackerConfig::default()).rx
}

pub(crate) fn start_observe_thread_controlled(
    observe_tick: Duration,
    initial: TrackerConfig,
) -> ObserveHandle {
    let (tx, rx) = mpsc::sync_channel::<observe::snapshot::OsSnapshot>(2);
    let control = ObserveControl::new(initial);
    let control_for_thread = control.clone();
    thread::spawn(move || {
        let fast = observe_tick.max(Duration::from_millis(500));
        let medium = (fast.saturating_mul(5)).max(Duration::from_secs(3));
        let slow = (fast.saturating_mul(15)).max(Duration::from_secs(10));
        let static_t = (fast.saturating_mul(30)).max(Duration::from_secs(20));

        let mut next_fast = Instant::now();
        let mut next_medium = Instant::now();
        let mut next_slow = Instant::now();
        let mut next_static = Instant::now();

        let mut snapshot = observe::snapshot::OsSnapshot::default();
        let mut gate = EmissionGate::new(Duration::from_secs(20));

        loop {
            let now = Instant::now();
            let mut touched = false;
            let tracker_cfg = control_for_thread.get();

            if now >= next_fast {
                if tracker_cfg.is_enabled("input") {
                    observe::input::poll(&mut snapshot);
                }
                if tracker_cfg.is_enabled("process") {
                    observe::process::poll(&mut snapshot);
                }
                if tracker_cfg.is_enabled("session") {
                    observe::session::poll(&mut snapshot);
                }
                next_fast = now + adaptive_fast_interval(fast, &snapshot);
                touched = true;
            }

            if now >= next_medium {
                if tracker_cfg.is_enabled("hardware") {
                    observe::hardware::poll(&mut snapshot);
                }
                if tracker_cfg.is_enabled("network") {
                    observe::network::poll(&mut snapshot);
                }
                if tracker_cfg.is_enabled("audio") {
                    observe::audio::poll(&mut snapshot);
                }
                if tracker_cfg.is_enabled("storage") {
                    observe::storage::poll(&mut snapshot);
                }
                next_medium = now + medium;
                touched = true;
            }

            if now >= next_slow {
                if tracker_cfg.is_enabled("calendar") {
                    observe::calendar::poll(&mut snapshot);
                }
                if tracker_cfg.is_enabled("environment") {
                    observe::environment::poll(&mut snapshot);
                }
                next_slow = now + slow;
                touched = true;
            }

            if now >= next_static {
                if tracker_cfg.is_enabled("accessibility") {
                    observe::accessibility::poll(&mut snapshot);
                }
                if tracker_cfg.is_enabled("peripherals") {
                    observe::peripherals::poll(&mut snapshot);
                }
                next_static = now + static_t;
                touched = true;
            }

            if !touched {
                let sleep_for = min_due(next_fast, next_medium, next_slow, next_static)
                    .saturating_duration_since(Instant::now());
                if !sleep_for.is_zero() {
                    thread::sleep(sleep_for);
                }
                continue;
            }

            snapshot.ts = unix_ts();
            if gate.should_emit(&snapshot) && coalesced_send(&tx, snapshot.clone()).is_err() {
                break;
            }

            let sleep_for = min_due(next_fast, next_medium, next_slow, next_static)
                .saturating_duration_since(Instant::now());
            if !sleep_for.is_zero() {
                thread::sleep(sleep_for);
            }
        }
    });
    ObserveHandle { rx, control }
}

fn coalesced_send(
    tx: &SyncSender<observe::snapshot::OsSnapshot>,
    snapshot: observe::snapshot::OsSnapshot,
) -> Result<(), ()> {
    match tx.try_send(snapshot) {
        Ok(()) => Ok(()),
        Err(TrySendError::Full(_)) => Ok(()),
        Err(TrySendError::Disconnected(_)) => Err(()),
    }
}

fn min_due(a: Instant, b: Instant, c: Instant, d: Instant) -> Instant {
    a.min(b).min(c).min(d)
}

fn adaptive_fast_interval(base: Duration, snapshot: &observe::snapshot::OsSnapshot) -> Duration {
    if snapshot.thermal_throttled || snapshot.cpu_pct >= 80.0 {
        Duration::from_secs(5)
    } else if snapshot.cpu_pct >= 60.0 {
        Duration::from_secs(3)
    } else {
        base
    }
}

#[derive(Debug)]
struct EmissionGate {
    last: Option<observe::snapshot::OsSnapshot>,
    heartbeat_every: Duration,
    next_heartbeat_at: Instant,
}

impl EmissionGate {
    fn new(heartbeat_every: Duration) -> Self {
        Self {
            last: None,
            heartbeat_every,
            next_heartbeat_at: Instant::now() + heartbeat_every,
        }
    }

    fn should_emit(&mut self, next: &observe::snapshot::OsSnapshot) -> bool {
        let heartbeat_due = Instant::now() >= self.next_heartbeat_at;
        let changed = match &self.last {
            None => true,
            Some(prev) => significant_change(prev, next),
        };
        if changed || heartbeat_due {
            self.last = Some(next.clone());
            self.next_heartbeat_at = Instant::now() + self.heartbeat_every;
            true
        } else {
            false
        }
    }
}

fn significant_change(
    prev: &observe::snapshot::OsSnapshot,
    next: &observe::snapshot::OsSnapshot,
) -> bool {
    if prev.active_app != next.active_app
        || prev.active_title != next.active_title
        || prev.active_interface != next.active_interface
        || prev.wifi_ssid != next.wifi_ssid
        || prev.network_up != next.network_up
        || prev.vpn_active != next.vpn_active
        || prev.charging != next.charging
        || prev.screen_locked != next.screen_locked
        || prev.screensaver_active != next.screensaver_active
        || prev.dark_mode != next.dark_mode
        || prev.night_shift_enabled != next.night_shift_enabled
        || prev.media_playing != next.media_playing
        || prev.output_muted != next.output_muted
        || prev.mic_active != next.mic_active
        || prev.headphones_connected != next.headphones_connected
        || prev.bluetooth_power_on != next.bluetooth_power_on
        || prev.time_machine_running != next.time_machine_running
        || prev.accessibility_enabled != next.accessibility_enabled
        || prev.voiceover_enabled != next.voiceover_enabled
    {
        return true;
    }

    if diff_f32(prev.cpu_pct, next.cpu_pct) >= 5.0
        || diff_f32(prev.mem_pct, next.mem_pct) >= 2.0
        || diff_opt_f32(prev.cpu_temp_c, next.cpu_temp_c) >= 2.0
        || diff_opt_i32(prev.wifi_rssi, next.wifi_rssi) >= 3
        || diff_opt_f32(prev.battery_pct, next.battery_pct) >= 1.0
        || prev.net_tx_kbps.abs_diff(next.net_tx_kbps) >= 10
        || prev.net_rx_kbps.abs_diff(next.net_rx_kbps) >= 10
        || diff_f32(prev.key_wpm, next.key_wpm) >= 5.0
        || diff_f32(prev.disk_used_pct, next.disk_used_pct) >= 1.0
    {
        return true;
    }

    if prev.idle_secs < 120 && next.idle_secs < 120 {
        if prev.idle_secs.abs_diff(next.idle_secs) >= 5 {
            return true;
        }
    } else if prev.idle_secs / 60 != next.idle_secs / 60 {
        return true;
    }

    if prev.process_count.abs_diff(next.process_count) >= 3
        || prev.top_process != next.top_process
        || diff_f32(prev.top_process_cpu_pct, next.top_process_cpu_pct) >= 10.0
        || prev.per_core_cpu_sig != next.per_core_cpu_sig
        || prev.memory_hungry_process_sig != next.memory_hungry_process_sig
    {
        return true;
    }

    false
}

fn diff_f32(a: f32, b: f32) -> f32 {
    (a - b).abs()
}

fn diff_opt_f32(a: Option<f32>, b: Option<f32>) -> f32 {
    match (a, b) {
        (Some(x), Some(y)) => (x - y).abs(),
        (None, None) => 0.0,
        _ => f32::MAX,
    }
}

fn diff_opt_i32(a: Option<i32>, b: Option<i32>) -> i32 {
    match (a, b) {
        (Some(x), Some(y)) => (x - y).abs(),
        (None, None) => 0,
        _ => i32::MAX,
    }
}

fn unix_ts() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
