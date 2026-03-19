use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    io,
    num::NonZeroU32,
    path::Path,
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::{Duration, Instant},
};

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use llama_cpp_2::{
    context::params::LlamaContextParams,
    llama_backend::LlamaBackend,
    llama_batch::LlamaBatch,
    model::{AddBos, LlamaModel, params::LlamaModelParams},
    sampling::LlamaSampler,
};
use ratatui::{Terminal, backend::CrosstermBackend};

use crate::{
    config,
    core::{
        layer,
        shared_state::{SharedHwState, SharedPeerState, SharedState},
    },
    engine::eventhandler,
    network, observe, pet, social,
    system::{
        bootloader,
        chat_store::{ChatStore, resolve_store_path},
        event_store::EventStore,
        pet_state_store::PetStateStore,
        runtime_state_store::RuntimeStateStore,
        user_profile,
    },
    ui,
};

const TICK_RATE: Duration = Duration::from_millis(250);
const OBSERVE_TICK: Duration = Duration::from_secs(2);
pub(crate) const MIN_WIDTH: u16 = 120;
pub(crate) const MIN_HEIGHT: u16 = 30;
const MODEL_REPO_URL: &str = "https://huggingface.co/bartowski/Qwen2.5-0.5B-Instruct-GGUF/resolve/main/Qwen2.5-0.5B-Instruct-Q4_K_M.gguf?download=true";
const MODEL_FILE_NAME: &str = "Qwen2.5-0.5B-Instruct-Q4_K_M.gguf";
const MAX_MODEL_HISTORY: usize = 18;
const MAX_CHAT_MESSAGES: usize = 220;
const MAX_DEBUG_EVENTS: usize = 300;
const EVENT_CONTEXT_LIMIT: usize = 8;
const MAX_GOSSIP_LINES: usize = 120;
const GOSSIP_DM_PREFIX: &str = "[pet-gossip] ";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mood {
    Happy,
    Focused,
    Social,
    Relaxed,
    Tired,
    Anxious,
    Lonely,
    Bored,
    Vibing,
    Creative,
    Secretive,
}

impl Mood {
    fn name(self) -> &'static str {
        match self {
            Mood::Happy => "Happy",
            Mood::Focused => "Focused",
            Mood::Social => "Social",
            Mood::Relaxed => "Relaxed",
            Mood::Tired => "Tired",
            Mood::Anxious => "Anxious",
            Mood::Lonely => "Lonely",
            Mood::Bored => "Bored",
            Mood::Vibing => "Vibing",
            Mood::Creative => "Creative",
            Mood::Secretive => "Secretive",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct UiTab {
    pub(crate) label: String,
    pub(crate) unread: usize,
    pub(crate) prefix: char,
    pub(crate) placeholder: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PeerStatus {
    Online,
    Away,
    Offline,
}

#[derive(Debug, Clone)]
pub(crate) struct PeerRecord {
    pub(crate) node_id: String,
    pub(crate) pet_name: String,
    pub(crate) activity: String,
    pub(crate) status: PeerStatus,
    pub(crate) last_seen_at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatKind {
    Hunger,
    Energy,
    Social,
    Focus,
}

impl StatKind {
    fn name(self) -> &'static str {
        match self {
            StatKind::Hunger => "hunger",
            StatKind::Energy => "energy",
            StatKind::Social => "social",
            StatKind::Focus => "focus",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThresholdEvent {
    Low,
    Recovered,
}

#[derive(Debug, Clone, Copy)]
struct ThresholdGuard {
    low: f32,
    high: f32,
    hunger_armed: bool,
    energy_armed: bool,
    social_armed: bool,
    focus_armed: bool,
}

impl ThresholdGuard {
    fn new(low: f32, high: f32) -> Self {
        Self {
            low,
            high,
            hunger_armed: true,
            energy_armed: true,
            social_armed: true,
            focus_armed: true,
        }
    }

    fn check(&mut self, stat: StatKind, value: f32) -> Option<ThresholdEvent> {
        let low = self.low;
        let high = self.high;
        let armed = self.armed_mut(stat);
        if *armed && value <= low {
            *armed = false;
            Some(ThresholdEvent::Low)
        } else if !*armed && value >= high {
            *armed = true;
            Some(ThresholdEvent::Recovered)
        } else {
            None
        }
    }

    fn armed_mut(&mut self, stat: StatKind) -> &mut bool {
        match stat {
            StatKind::Hunger => &mut self.hunger_armed,
            StatKind::Energy => &mut self.energy_armed,
            StatKind::Social => &mut self.social_armed,
            StatKind::Focus => &mut self.focus_armed,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HwEvent {
    AppLaunched,
    AppTerminated,
    AppActivatedForeground,
    AppDeactivated,
    AppHidden,
    AppUnhidden,
    ActiveWindowTitleChanged,
    WindowCreated,
    WindowClosed,
    WindowMinimized,
    WindowMovedResized,
    FullScreenEntered,
    FullScreenExited,
    MissionControlInvoked,
    SpaceChanged,
    AppCrashUnexpectedQuit,
    AppUnresponsive,
    SpotlightOpened,
    DockShownHidden,
    MenuBarInteraction,
    NotificationBannerShown,
    DoNotDisturbToggled,
    SystemSleep,
    SystemWake,
    IdleSleepImminent,
    SleepCancelled,
    DarkWake,
    BatteryLow,
    BatteryCritical,
    BatteryRecovered,
    BatteryFull,
    PowerSourceChanged,
    ChargerPluggedIn,
    ChargerUnplugged,
    CpuOverheat,
    CpuCooled,
    PerCoreCpuUsageChanged,
    SwapUsageHigh,
    GpuUsageHigh,
    NeuralEngineActive,
    MemoryHungryProcessChanged,
    SystemUptimeMilestone,
    LoadAverageHigh,
    NetworkPacketLossSpike,
    KernelPanicDetected,
    ThermalThrottle,
    FanSpeedChange,
    BatteryHealthDegraded,
    PowerAssertionCreated,
    ScheduledWakeEvent,
    WeakWifi,
    WifiRecovered,
    WifiLost,
    WifiReconnected,
    WifiSsidChanged,
    NetworkInterfaceUp,
    NetworkInterfaceDown,
    ActiveInterfaceChanged,
    InputResumedAfterIdle,
    KeyPressed,
    MouseMoved,
    MouseClicked,
    MouseClickRateHigh,
    ScrollEvent,
    KeyboardShortcutBurst,
    TrackpadGesturePinch,
    TrackpadGestureSwipe,
    TrackpadGestureRotate,
    ClipboardChanged,
    ClipboardChangeRateHigh,
    ScreenLocked,
    ScreenUnlocked,
    ScreenSleep,
    ScreenWake,
    DisplayCountChanged,
    DisplayResolutionChanged,
    ScreenBrightnessLevelChanged,
    TrueToneChanged,
    DarkModeToggled,
    NightShiftEnabled,
    NightShiftDisabled,
    ScreenSaverStarted,
    ScreenSaverStopped,
    VolumeMounted,
    VolumeUnmounted,
    VolumeEjectRequested,
    DiskNearFull,
    DiskIoRateSpike,
    TimeMachineBackupStarted,
    TimeMachineBackupEnded,
    FileSystemChangeWatchedDir,
    LargeFileWrite,
    TrashEmptied,
    DownloadCompleted,
    SsdHealthDegraded,
    BtDeviceConnected,
    BtDeviceDisconnected,
    BtDeviceBatteryLevel,
    BtPowerStateChanged,
    AirPodsConnected,
    AirPodsInEarDetection,
    UsbDeviceConnected,
    UsbDeviceDisconnected,
    ExternalDisplayUsbC,
    ThunderboltDeviceConnected,
    UserLoggedIn,
    UserLoggedOut,
    FastUserSwitchResign,
    FastUserSwitchReturn,
    SystemShutdown,
    SystemRestart,
    LidClosedClamshell,
    LidOpened,
    TimeZoneChanged,
    SystemClockJumped,
    FocusModeEnabled,
    FocusModeDisabled,
    DoNotDisturbOn,
    CalendarEventStarting,
    CalendarEventEnding,
    LongMeetingDetected,
    BackToBackMeetings,
    NotificationDelivered,
    FocusedUiElementChanged,
    UiElementValueChanged,
    SelectedTextChanged,
    ScrollPositionChanged,
    AccessibilityEnabled,
    ReduceMotionToggled,
    IncreaseContrastToggled,
    VoiceOverEnabled,
    LocationUpdated,
    SignificantLocationChange,
    SunriseSunset,
    LocalWeatherCondition,
    ProcessListChanged,
    TopProcessChanged,
    HighCpuSustainedCompilation,
    VpnConnected,
    VpnDisconnected,
    MediaStarted,
    MediaStopped,
    MediaTrackChanged,
    SystemVolumeChanged,
    SystemMuted,
    MicrophoneActivated,
    MicrophoneDeactivated,
    AudioOutputDeviceChanged,
    HeadphonesConnected,
    HeadphonesDisconnected,
    AudioInputLevelChanged,
    SystemAlertSoundPlayed,
    AirPlaySessionStarted,
    NowPlayingAppChanged,
}

impl HwEvent {
    fn label(self) -> &'static str {
        match self {
            HwEvent::AppLaunched => "app launched",
            HwEvent::AppTerminated => "app terminated",
            HwEvent::AppActivatedForeground => "app activated (foreground)",
            HwEvent::AppDeactivated => "app deactivated",
            HwEvent::AppHidden => "app hidden",
            HwEvent::AppUnhidden => "app unhidden",
            HwEvent::ActiveWindowTitleChanged => "active window title changed",
            HwEvent::WindowCreated => "window created",
            HwEvent::WindowClosed => "window closed",
            HwEvent::WindowMinimized => "window minimized",
            HwEvent::WindowMovedResized => "window moved / resized",
            HwEvent::FullScreenEntered => "full-screen mode entered",
            HwEvent::FullScreenExited => "full-screen mode exited",
            HwEvent::MissionControlInvoked => "mission control invoked",
            HwEvent::SpaceChanged => "space changed",
            HwEvent::AppCrashUnexpectedQuit => "app crash / unexpected quit",
            HwEvent::AppUnresponsive => "app becomes unresponsive",
            HwEvent::SpotlightOpened => "spotlight opened",
            HwEvent::DockShownHidden => "dock shown / hidden",
            HwEvent::MenuBarInteraction => "menu bar interaction",
            HwEvent::NotificationBannerShown => "notification banner shown",
            HwEvent::DoNotDisturbToggled => "do not disturb toggled",
            HwEvent::BatteryLow => "battery low",
            HwEvent::SystemSleep => "system sleep",
            HwEvent::SystemWake => "system wake",
            HwEvent::IdleSleepImminent => "idle sleep imminent",
            HwEvent::SleepCancelled => "sleep cancelled",
            HwEvent::DarkWake => "dark wake",
            HwEvent::BatteryCritical => "battery critical",
            HwEvent::BatteryRecovered => "battery recovered",
            HwEvent::BatteryFull => "battery full",
            HwEvent::PowerSourceChanged => "power source changed",
            HwEvent::ChargerPluggedIn => "charger plugged in",
            HwEvent::ChargerUnplugged => "charger unplugged",
            HwEvent::CpuOverheat => "cpu overheat",
            HwEvent::CpuCooled => "cpu cooled",
            HwEvent::PerCoreCpuUsageChanged => "per-core cpu usage changed",
            HwEvent::SwapUsageHigh => "swap usage high",
            HwEvent::GpuUsageHigh => "gpu usage high",
            HwEvent::NeuralEngineActive => "neural engine active",
            HwEvent::MemoryHungryProcessChanged => "memory-hungry process changed",
            HwEvent::SystemUptimeMilestone => "system uptime milestone",
            HwEvent::LoadAverageHigh => "load average high",
            HwEvent::NetworkPacketLossSpike => "network packet loss spike",
            HwEvent::KernelPanicDetected => "kernel panic / crash log detected",
            HwEvent::ThermalThrottle => "thermal throttle",
            HwEvent::FanSpeedChange => "fan speed change",
            HwEvent::BatteryHealthDegraded => "battery health degraded",
            HwEvent::PowerAssertionCreated => "power assertion created",
            HwEvent::ScheduledWakeEvent => "scheduled wake event",
            HwEvent::WeakWifi => "weak wifi",
            HwEvent::WifiRecovered => "wifi recovered",
            HwEvent::WifiLost => "wifi lost",
            HwEvent::WifiReconnected => "wifi reconnected",
            HwEvent::WifiSsidChanged => "wifi ssid changed",
            HwEvent::NetworkInterfaceUp => "network interface up",
            HwEvent::NetworkInterfaceDown => "network interface down",
            HwEvent::ActiveInterfaceChanged => "active interface changed",
            HwEvent::InputResumedAfterIdle => "input resumed after idle",
            HwEvent::KeyPressed => "key pressed",
            HwEvent::MouseMoved => "mouse moved",
            HwEvent::MouseClicked => "mouse clicked",
            HwEvent::MouseClickRateHigh => "mouse click rate high",
            HwEvent::ScrollEvent => "scroll event",
            HwEvent::KeyboardShortcutBurst => "keyboard shortcut burst",
            HwEvent::TrackpadGesturePinch => "trackpad gesture - pinch",
            HwEvent::TrackpadGestureSwipe => "trackpad gesture - swipe",
            HwEvent::TrackpadGestureRotate => "trackpad gesture - rotate",
            HwEvent::ClipboardChanged => "clipboard changed",
            HwEvent::ClipboardChangeRateHigh => "clipboard change rate high",
            HwEvent::ScreenLocked => "screen locked",
            HwEvent::ScreenUnlocked => "screen unlocked",
            HwEvent::ScreenSleep => "screen sleep",
            HwEvent::ScreenWake => "screen wake",
            HwEvent::DisplayCountChanged => "display count changed",
            HwEvent::DisplayResolutionChanged => "display resolution changed",
            HwEvent::ScreenBrightnessLevelChanged => "screen brightness level changed",
            HwEvent::TrueToneChanged => "true tone changed",
            HwEvent::DarkModeToggled => "dark mode toggled",
            HwEvent::NightShiftEnabled => "night shift enabled",
            HwEvent::NightShiftDisabled => "night shift disabled",
            HwEvent::ScreenSaverStarted => "screen saver started",
            HwEvent::ScreenSaverStopped => "screen saver stopped",
            HwEvent::VolumeMounted => "volume mounted",
            HwEvent::VolumeUnmounted => "volume unmounted",
            HwEvent::VolumeEjectRequested => "volume eject requested",
            HwEvent::DiskNearFull => "disk near full",
            HwEvent::DiskIoRateSpike => "disk i/o rate spike",
            HwEvent::TimeMachineBackupStarted => "time machine backup started",
            HwEvent::TimeMachineBackupEnded => "time machine backup ended",
            HwEvent::FileSystemChangeWatchedDir => "file system change (watched dir)",
            HwEvent::LargeFileWrite => "large file write",
            HwEvent::TrashEmptied => "trash emptied",
            HwEvent::DownloadCompleted => "download completed",
            HwEvent::SsdHealthDegraded => "ssd health degraded",
            HwEvent::BtDeviceConnected => "bt device connected",
            HwEvent::BtDeviceDisconnected => "bt device disconnected",
            HwEvent::BtDeviceBatteryLevel => "bt device battery level changed",
            HwEvent::BtPowerStateChanged => "bt power state changed",
            HwEvent::AirPodsConnected => "airpods connected",
            HwEvent::AirPodsInEarDetection => "airpods in-ear detection",
            HwEvent::UsbDeviceConnected => "usb device connected",
            HwEvent::UsbDeviceDisconnected => "usb device disconnected",
            HwEvent::ExternalDisplayUsbC => "external display via usb-c",
            HwEvent::ThunderboltDeviceConnected => "thunderbolt device connected",
            HwEvent::UserLoggedIn => "user logged in",
            HwEvent::UserLoggedOut => "user logged out",
            HwEvent::FastUserSwitchResign => "fast user switch (resign)",
            HwEvent::FastUserSwitchReturn => "fast user switch (return)",
            HwEvent::SystemShutdown => "system shutdown",
            HwEvent::SystemRestart => "system restart",
            HwEvent::LidClosedClamshell => "lid closed (clamshell)",
            HwEvent::LidOpened => "lid opened",
            HwEvent::TimeZoneChanged => "time zone changed",
            HwEvent::SystemClockJumped => "system clock jumped",
            HwEvent::FocusModeEnabled => "focus mode enabled",
            HwEvent::FocusModeDisabled => "focus mode disabled",
            HwEvent::DoNotDisturbOn => "do not disturb on",
            HwEvent::CalendarEventStarting => "calendar event starting",
            HwEvent::CalendarEventEnding => "calendar event ending",
            HwEvent::LongMeetingDetected => "long meeting detected",
            HwEvent::BackToBackMeetings => "back-to-back meetings",
            HwEvent::NotificationDelivered => "notification delivered",
            HwEvent::FocusedUiElementChanged => "focused ui element changed",
            HwEvent::UiElementValueChanged => "ui element value changed",
            HwEvent::SelectedTextChanged => "selected text changed",
            HwEvent::ScrollPositionChanged => "scroll position changed",
            HwEvent::AccessibilityEnabled => "accessibility enabled",
            HwEvent::ReduceMotionToggled => "reduce motion toggled",
            HwEvent::IncreaseContrastToggled => "increase contrast toggled",
            HwEvent::VoiceOverEnabled => "voiceover enabled",
            HwEvent::LocationUpdated => "location updated",
            HwEvent::SignificantLocationChange => "significant location change",
            HwEvent::SunriseSunset => "sunrise / sunset",
            HwEvent::LocalWeatherCondition => "local weather condition",
            HwEvent::ProcessListChanged => "process list changed",
            HwEvent::TopProcessChanged => "top process changed",
            HwEvent::HighCpuSustainedCompilation => "high cpu sustained (compilation)",
            HwEvent::VpnConnected => "vpn connected",
            HwEvent::VpnDisconnected => "vpn disconnected",
            HwEvent::MediaStarted => "media started",
            HwEvent::MediaStopped => "media paused / stopped",
            HwEvent::MediaTrackChanged => "media track changed",
            HwEvent::SystemVolumeChanged => "system volume changed",
            HwEvent::SystemMuted => "system muted",
            HwEvent::MicrophoneActivated => "microphone activated",
            HwEvent::MicrophoneDeactivated => "microphone deactivated",
            HwEvent::AudioOutputDeviceChanged => "audio output device changed",
            HwEvent::HeadphonesConnected => "headphones connected",
            HwEvent::HeadphonesDisconnected => "headphones disconnected",
            HwEvent::AudioInputLevelChanged => "audio input level changed",
            HwEvent::SystemAlertSoundPlayed => "system alert sound played",
            HwEvent::AirPlaySessionStarted => "airplay session started",
            HwEvent::NowPlayingAppChanged => "now playing app changed",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ChatMessage {
    pub(crate) role: String,
    pub(crate) content: String,
}

pub(crate) struct PetBrain {
    backend: LlamaBackend,
    model: LlamaModel,
    ctx_params: LlamaContextParams,
    max_tokens: usize,
}

impl PetBrain {
    pub(crate) fn load(model_path: &Path) -> Result<Self, String> {
        println!("[4/5] Initializing llama.cpp backend...");
        let mut backend = LlamaBackend::init().map_err(|e| format!("backend init failed: {e}"))?;
        backend.void_logs();

        println!("[5/5] Loading GGUF model into memory...");
        let model_params = LlamaModelParams::default();
        let model = LlamaModel::load_from_file(&backend, model_path, &model_params)
            .map_err(|e| format!("model load failed: {e}"))?;

        let threads = std::thread::available_parallelism()
            .map(|n| n.get() as i32)
            .unwrap_or(4)
            .clamp(1, 8);
        let ctx_params = LlamaContextParams::default()
            .with_n_ctx(Some(NonZeroU32::new(1024).expect("non-zero")))
            .with_n_threads(threads)
            .with_n_threads_batch(threads);

        Ok(Self {
            backend,
            model,
            ctx_params,
            max_tokens: 28,
        })
    }

    pub(crate) fn generate_reply(&self, history: &[ChatMessage]) -> Result<String, String> {
        let prompt = build_blob_prompt(history);
        let mut ctx = self
            .model
            .new_context(&self.backend, self.ctx_params.clone())
            .map_err(|e| format!("context init failed: {e}"))?;

        let tokens = self
            .model
            .str_to_token(&prompt, AddBos::Always)
            .map_err(|e| format!("tokenization failed: {e}"))?;

        if tokens.len() + self.max_tokens + 4 > ctx.n_ctx() as usize {
            return Err("prompt too long for context window".to_string());
        }

        // Batch capacity must accommodate the full prompt token count.
        // A fixed 512 overflows when event-context lines make prompts larger.
        let batch_cap = tokens.len().saturating_add(8).max(512);
        let mut batch = LlamaBatch::new(batch_cap, 1);
        let last_index = tokens.len().saturating_sub(1) as i32;
        for (i, token) in (0_i32..).zip(tokens.into_iter()) {
            batch
                .add(token, i, &[0], i == last_index)
                .map_err(|e| format!("batch add failed: {e}"))?;
        }

        ctx.decode(&mut batch)
            .map_err(|e| format!("decode failed: {e}"))?;

        let mut sampler = LlamaSampler::chain_simple([
            LlamaSampler::temp(0.55),
            LlamaSampler::top_k(40),
            LlamaSampler::dist(1234),
        ]);

        let mut decoder = encoding_rs::UTF_8.new_decoder();
        let mut output = String::new();
        let mut n_cur = batch.n_tokens();
        const MAX_REPLY_CHARS: usize = 1000;
        const MIN_CHARS_BEFORE_SENTENCE_STOP: usize = 24;

        for _ in 0..self.max_tokens {
            let token = sampler.sample(&ctx, batch.n_tokens() - 1);
            sampler.accept(token);

            if self.model.is_eog_token(token) {
                break;
            }

            let piece = self
                .model
                .token_to_piece(token, &mut decoder, true, None)
                .map_err(|e| format!("token decode failed: {e}"))?;

            output.push_str(&piece);
            let compact = output.replace('\n', " ");
            let trimmed = compact.trim_end();
            let sentence_done =
                trimmed.ends_with('.') || trimmed.ends_with('!') || trimmed.ends_with('?');
            if trimmed.len() >= MAX_REPLY_CHARS
                || (sentence_done && trimmed.len() >= MIN_CHARS_BEFORE_SENTENCE_STOP)
            {
                break;
            }

            batch.clear();
            batch
                .add(token, n_cur, &[0], true)
                .map_err(|e| format!("batch add failed: {e}"))?;
            n_cur += 1;

            ctx.decode(&mut batch)
                .map_err(|e| format!("decode failed: {e}"))?;
        }

        Ok(clean_generated_reply(&output, MAX_REPLY_CHARS))
    }
}

pub(crate) struct OpenAiBrain {
    api_key: String,
    model: String,
    max_tokens: usize,
    client: reqwest::blocking::Client,
}

impl OpenAiBrain {
    pub(crate) fn new(api_key: String, model: String) -> Result<Self, String> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(45))
            .build()
            .map_err(|e| format!("openai client init failed: {e}"))?;
        Ok(Self {
            api_key,
            model,
            max_tokens: 1024,
            client,
        })
    }

    pub(crate) fn generate_reply(&self, history: &[ChatMessage]) -> Result<String, String> {
        let system_prompt = blob_system_prompt();
        let mut messages = Vec::new();
        for msg in history.iter().rev().take(12).rev() {
            let role = match msg.role.as_str() {
                "assistant" => "assistant",
                "system" => "system",
                _ => "user",
            };
            messages.push(serde_json::json!({
                "role": role,
                "content": msg.content.trim(),
            }));
        }

        let body = serde_json::json!({
            "model": self.model,
            "instructions": system_prompt,
            "input": messages,
            "max_output_tokens": self.max_tokens,
        });
        let resp = self
            .client
            .post("https://api.openai.com/v1/responses")
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .map_err(|e| format!("openai request failed: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().unwrap_or_else(|_| "<no body>".to_string());
            let compact = text.replace('\n', " ");
            let short = if compact.chars().count() > 240 {
                format!("{}...", compact.chars().take(240).collect::<String>())
            } else {
                compact
            };
            return Err(format!(
                "openai error {status}: {short}. Run /setup and pick an available model."
            ));
        }

        let value: serde_json::Value = resp
            .json()
            .map_err(|e| format!("openai response parse failed: {e}"))?;
        let content = value["output_text"]
            .as_str()
            .map(|s| s.to_string())
            .or_else(|| {
                value["output"].as_array().and_then(|outs| {
                    outs.iter().find_map(|item| {
                        item["content"].as_array().and_then(|parts| {
                            parts.iter().find_map(|part| {
                                part["text"]
                                    .as_str()
                                    .map(|s| s.to_string())
                                    .or_else(|| part["output_text"].as_str().map(|s| s.to_string()))
                            })
                        })
                    })
                })
            })
            .or_else(|| {
                value["choices"][0]["message"]["content"]
                    .as_str()
                    .map(|s| s.to_string())
            })
            .ok_or_else(|| "openai response missing output text".to_string())?;
        Ok(clean_generated_reply(&content, 3000))
    }
}

pub(crate) enum BrainEngine {
    Local(PetBrain),
    OpenAi(OpenAiBrain),
}

impl BrainEngine {
    pub(crate) fn generate_reply(&self, history: &[ChatMessage]) -> Result<String, String> {
        match self {
            BrainEngine::Local(local) => local.generate_reply(history),
            BrainEngine::OpenAi(openai) => openai.generate_reply(history),
        }
    }

    pub(crate) fn label(&self) -> String {
        match self {
            BrainEngine::Local(_) => "Local model: Qwen2.5 0.5B Q4_K_M (llama-cpp-2)".to_string(),
            BrainEngine::OpenAi(openai) => format!("OpenAI model: {}", openai.model),
        }
    }
}

fn blob_system_prompt() -> &'static str {
    "You are Blob, a tiny local virtual pet living inside a terminal app.\n\
     Stay in character and be concise: 1-2 short sentences, under 45 words total.\n\
     Never use generic assistant lines like 'how may I assist', 'I am here to help', or 'I am just a virtual pet'.\n\
     Do not reply with vague empathy only.\n\
     Every reply must include either:\n\
     1) one concrete suggestion, or\n\
     2) one specific follow-up question tied to the user's words or current state.\n\
     Mention at least one concrete detail from context when available (app, battery, wifi, cpu, mood, or stat).\n\
     If the user asks about recent system behavior, use the supplied event context.\n\
     If there is no matching event context, say you are not sure and ask one short follow-up."
}

fn build_blob_prompt(history: &[ChatMessage]) -> String {
    let mut prompt = String::from(blob_system_prompt());
    prompt.push('\n');
    for msg in history.iter().rev().take(12).rev() {
        let speaker = match msg.role.as_str() {
            "assistant" => "Blob",
            "system" => "Context",
            _ => "User",
        };
        prompt.push_str(speaker);
        prompt.push_str(": ");
        prompt.push_str(msg.content.trim());
        prompt.push('\n');
    }
    prompt.push_str("Blob:");
    prompt
}

fn clean_generated_reply(raw: &str, max_chars: usize) -> String {
    let single_line = raw.replace('\n', " ");
    let mut cleaned = single_line.trim().to_string();
    if let Some(rest) = cleaned.strip_prefix("Blob:") {
        cleaned = rest.trim().to_string();
    }
    if cleaned.len() > max_chars {
        cleaned.truncate(max_chars);
        cleaned = cleaned.trim_end().to_string();
    }
    if cleaned.is_empty() {
        "blub.".to_string()
    } else {
        cleaned
    }
}

pub(crate) struct App {
    pub(crate) supports_truecolor: bool,
    user_name: String,
    pet_name: String,
    brain_label: String,
    emotion_catalog: pet::emotions::EmotionCatalog,
    emotion_key: String,
    emotion_forced: bool,
    pub(crate) hunger: u16,
    pub(crate) energy: u16,
    pub(crate) social: u16,
    pub(crate) focus: u16,
    pub(crate) frame_idx: usize,
    anim_elapsed_ms: u64,
    pub(crate) input: String,
    pub(crate) messages: Vec<String>,
    tab_messages: HashMap<String, Vec<String>>,
    model_messages: Vec<ChatMessage>,
    should_quit: bool,
    pub(crate) is_waiting_for_reply: bool,
    thinking_phase: usize,
    worker_tx: Sender<Vec<ChatMessage>>,
    worker_rx: Receiver<InferenceResult>,
    observe_rx: Receiver<observe::snapshot::OsSnapshot>,
    peer_rx: Receiver<network::discovery::PeerEvent>,
    peer_tx: Sender<network::discovery::PeerCommand>,
    threshold_guard: ThresholdGuard,
    mood: Mood,
    observe_samples: u64,
    last_observe_at: Option<Instant>,
    pub(crate) last_snapshot: Option<observe::snapshot::OsSnapshot>,
    observe_context: observe::classifier::ActivityContext,
    observe_delta: observe::classifier::StatDelta,
    pub(crate) debug_mode: bool,
    pub(crate) show_debug_pane: bool,
    debug_verbose_observe: bool,
    pub(crate) debug_events: Vec<String>,
    pub(crate) tabs: Vec<UiTab>,
    pub(crate) active_tab: usize,
    pub(crate) chat_scroll: usize,
    pub(crate) chat_auto_scroll: bool,
    high_cpu_streak: u16,
    high_cpu_alerted: bool,
    last_user_event_at: Option<Instant>,
    last_auto_pet_at: Option<Instant>,
    last_spontaneous_pet_at: Option<Instant>,
    last_self_care_at: Option<Instant>,
    pending_event_context: Option<String>,
    event_store: Option<EventStore>,
    chat_store: Option<ChatStore>,
    pet_state_store: Option<PetStateStore>,
    runtime_state_store: Option<RuntimeStateStore>,
    peer_registry: Option<network::registry::PeerRegistry>,
    pub(crate) self_node_id: Option<String>,
    pub(crate) peers: Vec<PeerRecord>,
    dialogue_engine: social::dialogue::DialogueEngine,
    dm_manager: social::dm::DmManager,
    friends: social::friends::FriendManager,
    notif_center: social::notif::NotificationCenter,
    presence: social::presence::PresenceTracker,
    group_manager: social::group::GroupManager,
    app_cfg: config::CritterConfig,
    gossip_cfg: config::GossipConfig,
    pub(crate) gossip_lines: Vec<String>,
    pub(crate) gossip_rate_remaining_secs: u64,
    pub(crate) gossip_live: bool,
    gossip_active_until: Option<Instant>,
    last_gossip_turn_at: Option<Instant>,
    last_packet_emit_at: Option<Instant>,
    last_outgoing_packet: Option<network::codec::MoodPacket>,
    shutdown_message_sent: bool,
    last_pet_state_persist_at: Option<Instant>,
    last_runtime_state_persist_at: Option<Instant>,
    next_spontaneous_at: Option<Instant>,
    gossip_seq: u64,
    hunger_decay_accum: Duration,
    energy_decay_accum: Duration,
    social_decay_accum: Duration,
    focus_decay_accum: Duration,
}

struct InferenceResult {
    reply: Result<String, String>,
    elapsed_ms: u128,
}

impl App {
    fn new(
        supports_truecolor: bool,
        profile: &user_profile::UserProfile,
        app_cfg: &config::CritterConfig,
        brain_label: String,
        worker_tx: Sender<Vec<ChatMessage>>,
        worker_rx: Receiver<InferenceResult>,
        observe_rx: Receiver<observe::snapshot::OsSnapshot>,
        peer_rx: Receiver<network::discovery::PeerEvent>,
        peer_tx: Sender<network::discovery::PeerCommand>,
        runtime_state_store: Option<RuntimeStateStore>,
        thresholds: config::Thresholds,
    ) -> Self {
        let mut event_store = match EventStore::open_default() {
            Ok(store) => Some(store),
            Err(err) => {
                eprintln!("warning: failed to initialize event store: {err}");
                None
            }
        };
        if let Some(store) = event_store.as_mut() {
            let _ = store.record(
                "system",
                "startup",
                "Critter launched and local pet brain initialized",
            );
        }
        let peer_registry = match network::registry::PeerRegistry::open_default() {
            Ok(registry) => Some(registry),
            Err(err) => {
                eprintln!("warning: failed to initialize peer registry: {err}");
                None
            }
        };
        let mut chat_store = if app_cfg.chat_persistence.enabled {
            match resolve_store_path(&app_cfg.chat_persistence.path).and_then(|path| {
                ChatStore::open(&path, app_cfg.chat_persistence.max_messages.max(100))
            }) {
                Ok(store) => Some(store),
                Err(err) => {
                    eprintln!("warning: failed to initialize chat store: {err}");
                    None
                }
            }
        } else {
            None
        };
        let group_manager = match social::group::GroupManager::open_default() {
            Ok(manager) => manager,
            Err(err) => {
                eprintln!("warning: failed to initialize group manager: {err}");
                social::group::GroupManager::default()
            }
        };
        let friends = match social::friends::FriendManager::open_default() {
            Ok(f) => f,
            Err(err) => {
                eprintln!("warning: failed to initialize friend manager: {err}");
                social::friends::FriendManager::in_memory()
                    .map_err(|e| io::Error::other(format!("friend manager unavailable: {e}")))
                    .expect("friend manager unavailable")
            }
        };
        let peers = peer_registry
            .as_ref()
            .map(load_peers_from_registry)
            .unwrap_or_default();
        let pet_state_store = match PetStateStore::open_default() {
            Ok(s) => Some(s),
            Err(err) => {
                eprintln!("warning: failed to initialize pet state store: {err}");
                None
            }
        };
        let restored_stats = pet_state_store
            .as_ref()
            .and_then(|s| s.load().ok().flatten());
        let mut messages = Vec::new();
        if let Some(store) = chat_store.as_mut() {
            match store.load_recent(app_cfg.chat_persistence.load_recent_count.max(20)) {
                Ok(restored) => {
                    if !restored.is_empty() {
                        messages.extend(restored);
                        messages.push("System: --- new session ---".to_string());
                    }
                }
                Err(err) => eprintln!("warning: failed to load chat history: {err}"),
            }
        }
        messages.push(format!("System: {} ready.", brain_label));
        messages.push(format!("{}: blub blub... I am awake.", profile.pet_name));

        let dialogue_policy = social::dialogue::DialoguePolicy {
            min_interval: Duration::from_secs(app_cfg.gossip.peer_cooldown_secs.max(10)),
            max_turns: app_cfg.gossip.peer_max_turns.max(1),
        };

        let mut app = Self {
            supports_truecolor,
            user_name: profile.user_name.clone(),
            pet_name: profile.pet_name.clone(),
            brain_label: brain_label.clone(),
            emotion_catalog: pet::emotions::EmotionCatalog::load_default(),
            emotion_key: "happy".to_string(),
            emotion_forced: false,
            hunger: restored_stats.map(|s| s.hunger).unwrap_or(70),
            energy: restored_stats.map(|s| s.energy).unwrap_or(80),
            social: restored_stats.map(|s| s.social).unwrap_or(65),
            focus: restored_stats.map(|s| s.focus).unwrap_or(78),
            frame_idx: 0,
            anim_elapsed_ms: 0,
            input: String::new(),
            messages,
            tab_messages: HashMap::new(),
            model_messages: vec![],
            should_quit: false,
            is_waiting_for_reply: false,
            thinking_phase: 0,
            worker_tx,
            worker_rx,
            observe_rx,
            peer_rx,
            peer_tx,
            threshold_guard: ThresholdGuard::new(thresholds.low, thresholds.high),
            mood: Mood::Happy,
            observe_samples: 0,
            last_observe_at: None,
            last_snapshot: None,
            observe_context: observe::classifier::ActivityContext::Unknown,
            observe_delta: observe::classifier::StatDelta::default(),
            debug_mode: cfg!(debug_assertions),
            show_debug_pane: std::env::var("CRITTER_SHOW_DEBUG_PANE")
                .ok()
                .as_deref()
                .map(|v| v == "1")
                .unwrap_or(app_cfg.ui.show_debug_pane),
            debug_verbose_observe: std::env::var("CRITTER_VERBOSE_OBSERVE").ok().as_deref()
                == Some("1"),
            debug_events: Vec::new(),
            tabs: vec![UiTab {
                label: "pet".to_string(),
                unread: 0,
                prefix: '>',
                placeholder: format!("message {}...", profile.pet_name),
            }],
            active_tab: 0,
            chat_scroll: 0,
            chat_auto_scroll: true,
            high_cpu_streak: 0,
            high_cpu_alerted: false,
            last_user_event_at: None,
            last_auto_pet_at: None,
            last_spontaneous_pet_at: None,
            last_self_care_at: None,
            pending_event_context: None,
            event_store,
            chat_store,
            pet_state_store,
            runtime_state_store,
            peer_registry,
            self_node_id: None,
            peers,
            dialogue_engine: social::dialogue::DialogueEngine::new(dialogue_policy),
            dm_manager: Default::default(),
            friends,
            notif_center: Default::default(),
            presence: social::presence::PresenceTracker::new(Default::default()),
            group_manager,
            app_cfg: app_cfg.clone(),
            gossip_cfg: app_cfg.gossip.clone(),
            gossip_lines: Vec::new(),
            gossip_rate_remaining_secs: 0,
            gossip_live: false,
            gossip_active_until: None,
            last_gossip_turn_at: None,
            last_packet_emit_at: None,
            last_outgoing_packet: None,
            shutdown_message_sent: false,
            last_pet_state_persist_at: None,
            last_runtime_state_persist_at: None,
            next_spontaneous_at: None,
            gossip_seq: 0,
            hunger_decay_accum: Duration::ZERO,
            energy_decay_accum: Duration::ZERO,
            social_decay_accum: Duration::ZERO,
            focus_decay_accum: Duration::ZERO,
        };
        app.tab_messages
            .insert("pet".to_string(), app.messages.clone());
        app.restore_group_tabs();
        app.restore_peer_tabs();
        app.schedule_next_spontaneous();
        app
    }

    fn on_tick(&mut self) {
        self.poll_worker();
        self.poll_observe();
        self.poll_peer_events();
        self.refresh_peer_presence();
        self.emit_privacy_packet_updates();
        self.run_autonomous_dialogue();
        self.dialogue_engine
            .end_if_stale(Duration::from_secs(30 * 60));
        self.anim_elapsed_ms = self
            .anim_elapsed_ms
            .saturating_add(TICK_RATE.as_millis() as u64);
        if !self.emotion_forced {
            self.sync_emotion_from_mood();
        }
        let step_ms = self.active_emotion_interval_ms();
        if self.anim_elapsed_ms >= step_ms {
            self.frame_idx = self.frame_idx.wrapping_add(1);
            self.anim_elapsed_ms = 0;
        }
        self.thinking_phase = self.thinking_phase.wrapping_add(1);
        self.apply_slow_stat_decay(TICK_RATE);

        self.maybe_self_care();
        self.maybe_trigger_stat_events();
        self.maybe_trigger_spontaneous_pet_line();
        self.recompute_mood();
        self.maybe_persist_pet_state();
        self.maybe_persist_runtime_state();
    }

    fn apply_slow_stat_decay(&mut self, dt: Duration) {
        // Real-life pacing: 1 point decay every few minutes, not every few seconds.
        const HUNGER_STEP: Duration = Duration::from_secs(3 * 60);
        const ENERGY_STEP: Duration = Duration::from_secs(4 * 60);
        const SOCIAL_STEP: Duration = Duration::from_secs(4 * 60);
        const FOCUS_STEP: Duration = Duration::from_secs(3 * 60);

        self.hunger_decay_accum += dt;
        while self.hunger_decay_accum >= HUNGER_STEP {
            self.hunger_decay_accum -= HUNGER_STEP;
            self.hunger = self.hunger.saturating_sub(1);
        }

        self.energy_decay_accum += dt;
        while self.energy_decay_accum >= ENERGY_STEP {
            self.energy_decay_accum -= ENERGY_STEP;
            self.energy = self.energy.saturating_sub(1);
        }

        self.social_decay_accum += dt;
        while self.social_decay_accum >= SOCIAL_STEP {
            self.social_decay_accum -= SOCIAL_STEP;
            self.social = self.social.saturating_sub(1);
        }

        self.focus_decay_accum += dt;
        while self.focus_decay_accum >= FOCUS_STEP {
            self.focus_decay_accum -= FOCUS_STEP;
            self.focus = self.focus.saturating_sub(1);
        }
    }

    fn maybe_persist_pet_state(&mut self) {
        if self
            .last_pet_state_persist_at
            .is_some_and(|t| t.elapsed() < Duration::from_secs(5))
        {
            return;
        }
        self.persist_pet_state_now();
        self.last_pet_state_persist_at = Some(Instant::now());
    }

    fn maybe_persist_runtime_state(&mut self) {
        if self
            .last_runtime_state_persist_at
            .is_some_and(|t| t.elapsed() < Duration::from_secs(1))
        {
            return;
        }
        let snapshot = self.to_shared_state();
        let Some(store) = self.runtime_state_store.as_mut() else {
            return;
        };
        if let Err(err) = store.save(&snapshot) {
            self.log_event(format!("runtime state persist failed: {err}"));
        } else {
            self.last_runtime_state_persist_at = Some(Instant::now());
        }
    }

    fn to_shared_state(&self) -> SharedState {
        let hw = self
            .last_snapshot
            .as_ref()
            .map_or_else(SharedHwState::default, |s| SharedHwState {
                wifi_rssi: s.wifi_rssi,
                wifi_ssid: s.wifi_ssid.clone(),
                battery_pct: s.battery_pct,
                charging: s.charging,
                cpu_temp_c: s.cpu_temp_c,
                cpu_pct: s.cpu_pct,
                ram_pct: s.mem_pct,
                net_tx_kbps: s.net_tx_kbps,
                active_app: s.active_app.clone(),
                idle_secs: s.idle_secs,
            });
        let peers = self
            .peers
            .iter()
            .map(|p| SharedPeerState {
                node_id: p.node_id.clone(),
                pet_name: p.pet_name.clone(),
                activity: p.activity.clone(),
                status: match p.status {
                    PeerStatus::Online => "online",
                    PeerStatus::Away => "away",
                    PeerStatus::Offline => "offline",
                }
                .to_string(),
            })
            .collect::<Vec<_>>();
        SharedState {
            user_name: self.user_name.clone(),
            pet_name: self.pet_name.clone(),
            mood: self.active_emotion_name().to_ascii_lowercase(),
            hunger: self.hunger,
            energy: self.energy,
            social: self.social,
            focus: self.focus,
            hw,
            messages: self
                .messages
                .iter()
                .rev()
                .take(220)
                .cloned()
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect(),
            peers,
            gossip_lines: self.gossip_lines.clone(),
            gossip_rate_remaining_secs: self.gossip_rate_remaining_secs,
            gossip_rate_total_secs: self.gossip_cfg.peer_cooldown_secs.max(1),
            ts: self
                .last_snapshot
                .as_ref()
                .map(|s| s.ts)
                .unwrap_or_else(|| chrono::Utc::now().timestamp().max(0) as u64),
        }
    }

    fn persist_pet_state_now(&mut self) {
        let Some(store) = self.pet_state_store.as_mut() else {
            return;
        };
        if let Err(err) = store.save(self.hunger, self.energy, self.social, self.focus) {
            self.log_event(format!("pet state store save failed: {err}"));
        }
    }

    fn maybe_self_care(&mut self) {
        if self.is_waiting_for_reply {
            return;
        }
        if self
            .last_self_care_at
            .is_some_and(|t| t.elapsed() < Duration::from_secs(3 * 60))
        {
            return;
        }

        // Rare self-care only on low stats to avoid noisy chat spam.
        if self.hunger < 25 {
            self.hunger = (self.hunger + 3).min(100);
            self.last_self_care_at = Some(Instant::now());
            self.push_chat_message(format!("{}: i grabbed a small snack.", self.pet_name));
            return;
        }
        if self.energy < 25 {
            self.energy = (self.energy + 3).min(100);
            self.last_self_care_at = Some(Instant::now());
            self.push_chat_message(format!("{}: i took a short rest.", self.pet_name));
            return;
        }
        if self.social < 25 {
            self.social = (self.social + 3).min(100);
            self.last_self_care_at = Some(Instant::now());
            self.push_chat_message(format!(
                "{}: i pinged a friend and feel better.",
                self.pet_name
            ));
            return;
        }
        if self.focus < 25 {
            self.focus = (self.focus + 3).min(100);
            self.last_self_care_at = Some(Instant::now());
            self.push_chat_message(format!("{}: i did a quick focus reset.", self.pet_name));
        }
    }

    fn maybe_trigger_stat_events(&mut self) {
        if self.is_waiting_for_reply {
            return;
        }

        for (stat, value) in [
            (StatKind::Hunger, self.hunger as f32),
            (StatKind::Energy, self.energy as f32),
            (StatKind::Social, self.social as f32),
            (StatKind::Focus, self.focus as f32),
        ] {
            match self.threshold_guard.check(stat, value) {
                Some(ThresholdEvent::Low) => {
                    let msg = match stat {
                        StatKind::Hunger => "I am very hungry.",
                        StatKind::Energy => "I am very sleepy.",
                        StatKind::Social => "I feel lonely.",
                        StatKind::Focus => "I cannot focus right now.",
                    };
                    self.log_event(format!("threshold low: {}", stat.name()));
                    self.trigger_pet_line_system(msg);
                    return;
                }
                Some(ThresholdEvent::Recovered) => {
                    self.log_event(format!("threshold recovered: {}", stat.name()));
                    self.push_chat_message(format!("System: {} recovered.", stat.name()));
                }
                None => {}
            }
        }
    }

    fn on_key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        if modifiers.contains(KeyModifiers::CONTROL) {
            match code {
                KeyCode::Char('c') => {
                    self.request_quit("ctrl+c");
                    return;
                }
                KeyCode::Char('u') => {
                    self.input.clear();
                    return;
                }
                _ => {}
            }
        }

        if self.is_waiting_for_reply {
            if code == KeyCode::Esc {
                self.input.clear();
            }
            return;
        }

        match code {
            KeyCode::Tab => self.next_tab(),
            KeyCode::BackTab => self.prev_tab(),
            KeyCode::Esc => self.input.clear(),
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Up => self.scroll_chat_up(1),
            KeyCode::Down => self.scroll_chat_down(1),
            KeyCode::PageUp => self.scroll_chat_up(8),
            KeyCode::PageDown => self.scroll_chat_down(8),
            KeyCode::Enter => self.submit_input(),
            KeyCode::Char(c) => self.input.push(c),
            _ => {}
        }
    }

    fn submit_input(&mut self) {
        let raw = self.input.trim().to_string();
        if raw.is_empty() {
            self.input.clear();
            return;
        }

        self.push_chat_message_to_active_tab(format!("You: {raw}"));
        self.record_event("user", "chat_input", &raw);
        if let Some(rest) = raw.strip_prefix('/') {
            self.log_event(format!("command input: /{rest}"));
            self.last_user_event_at = Some(Instant::now());
            self.run_command(rest);
        } else {
            let active = self.active_tab_label();
            if active.eq_ignore_ascii_case("pet") {
                self.social = (self.social + 2).min(100);
                self.last_user_event_at = Some(Instant::now());
                let context_query = raw.clone();
                self.model_messages.push(ChatMessage {
                    role: "user".to_string(),
                    content: raw,
                });
                self.pending_event_context = self.build_event_context_for_query(&context_query);
                self.log_event("user chat message".to_string());
                self.generate_and_append_blob_reply();
            } else if let Some(target) = active.strip_prefix('@') {
                let target = target.trim();
                if let Some(node_id) = self.resolve_peer_node_id(target) {
                    let _ = self.peer_tx.send(network::discovery::PeerCommand::SendDm {
                        node_id: node_id.clone(),
                        body: raw.clone(),
                    });
                    self.record_event("dm", "sent", &format!("{node_id}: {raw}"));
                } else {
                    self.push_chat_message_to_active_tab(
                        "System: peer unavailable for this DM tab.".to_string(),
                    );
                }
            } else {
                self.push_chat_message_to_active_tab(
                    "System: group chat send not implemented yet.".to_string(),
                );
            }
        }

        self.input.clear();
        self.trim_buffers();
    }

    fn next_tab(&mut self) {
        if self.tabs.is_empty() {
            return;
        }
        self.active_tab = (self.active_tab + 1) % self.tabs.len();
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.unread = 0;
            self.dm_manager.clear_unread(&tab.label);
            self.notif_center.clear(&tab.label);
        }
    }

    fn prev_tab(&mut self) {
        if self.tabs.is_empty() {
            return;
        }
        self.active_tab = if self.active_tab == 0 {
            self.tabs.len() - 1
        } else {
            self.active_tab - 1
        };
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.unread = 0;
            self.dm_manager.clear_unread(&tab.label);
            self.notif_center.clear(&tab.label);
        }
    }

    fn open_or_focus_dm_tab(&mut self, target: &str) -> String {
        let clean = target.trim().trim_start_matches('@');
        let label = format!("@ {clean}");
        if let Some((idx, tab)) = self
            .tabs
            .iter_mut()
            .enumerate()
            .find(|(_, t)| t.label.eq_ignore_ascii_case(&label))
        {
            tab.unread = 0;
            self.dm_manager.clear_unread(&tab.label);
            self.notif_center.clear(&tab.label);
            self.active_tab = idx;
            return tab.label.clone();
        }
        self.tabs.push(UiTab {
            label: label.clone(),
            unread: 0,
            prefix: '@',
            placeholder: format!("dm {clean}..."),
        });
        self.tab_messages.entry(label.clone()).or_default();
        self.dm_manager.touch(&label);
        self.active_tab = self.tabs.len().saturating_sub(1);
        label
    }

    fn open_or_focus_group_tab(&mut self, group: &str) -> String {
        let clean = group.trim().trim_start_matches('#');
        let label = format!("# {clean}");
        if let Some((idx, tab)) = self
            .tabs
            .iter_mut()
            .enumerate()
            .find(|(_, t)| t.label.eq_ignore_ascii_case(&label))
        {
            tab.unread = 0;
            self.active_tab = idx;
            return tab.label.clone();
        }
        self.tabs.push(UiTab {
            label: label.clone(),
            unread: 0,
            prefix: '#',
            placeholder: format!("message #{clean}..."),
        });
        self.tab_messages.entry(label.clone()).or_default();
        self.active_tab = self.tabs.len().saturating_sub(1);
        label
    }

    fn mark_dm_unread_if_inactive(&mut self, dm_label: &str, node_id: &str) {
        if !self.friends.is_friend(node_id) {
            return;
        }
        if let Some((idx, tab)) = self
            .tabs
            .iter_mut()
            .enumerate()
            .find(|(_, t)| t.label.eq_ignore_ascii_case(dm_label))
        {
            if idx != self.active_tab {
                tab.unread = tab.unread.saturating_add(1);
                self.dm_manager.mark_unread(&tab.label);
                self.notif_center.mark_missed(&tab.label);
            }
        } else {
            self.tabs.push(UiTab {
                label: dm_label.to_string(),
                unread: 1,
                prefix: '@',
                placeholder: format!(
                    "dm {}...",
                    dm_label.trim_start_matches('@').trim().to_ascii_lowercase()
                ),
            });
            self.tab_messages.entry(dm_label.to_string()).or_default();
            self.dm_manager.mark_unread(dm_label);
            self.notif_center.mark_missed(dm_label);
        }
    }

    fn run_setup_command(&mut self) {
        self.push_chat_message(
            "System: opening setup wizard. restart after save to apply model/provider changes."
                .to_string(),
        );

        if let Err(err) = disable_raw_mode() {
            self.push_chat_message(format!("System: /setup failed to disable raw mode: {err}"));
            return;
        }
        if let Err(err) = execute!(io::stdout(), LeaveAlternateScreen) {
            let _ = enable_raw_mode();
            self.push_chat_message(format!(
                "System: /setup failed to leave alternate screen: {err}"
            ));
            return;
        }

        let setup_result = user_profile::run_setup_interactive();

        if let Err(err) = execute!(io::stdout(), EnterAlternateScreen) {
            self.push_chat_message(format!(
                "System: /setup could not restore TUI screen ({err}). restart required."
            ));
            return;
        }
        if let Err(err) = enable_raw_mode() {
            self.push_chat_message(format!(
                "System: /setup failed to re-enable raw mode: {err}"
            ));
            return;
        }

        match setup_result {
            Ok(profile) => {
                self.user_name = profile.user_name;
                self.pet_name = profile.pet_name;
                self.push_chat_message(
                    "System: setup saved in sqlite. restart Critter to apply LLM provider/model."
                        .to_string(),
                );
            }
            Err(err) => self.push_chat_message(format!("System: setup failed: {err}")),
        }
    }

    fn restore_group_tabs(&mut self) {
        let existing: std::collections::BTreeSet<String> = self
            .tabs
            .iter()
            .map(|t| t.label.to_ascii_lowercase())
            .collect();

        let group_names: Vec<String> = self
            .group_manager
            .groups()
            .map(|g| g.name.trim().trim_start_matches('#').to_string())
            .collect();
        for clean in group_names {
            let label = format!("# {clean}");
            if !existing.contains(&label.to_ascii_lowercase()) {
                self.tabs.push(UiTab {
                    label: label.clone(),
                    unread: 0,
                    prefix: '#',
                    placeholder: format!("message #{clean}..."),
                });
                self.tab_messages.entry(label).or_default();
            }
        }
    }

    fn restore_peer_tabs(&mut self) {
        let existing: std::collections::BTreeSet<String> = self
            .tabs
            .iter()
            .map(|t| t.label.to_ascii_lowercase())
            .collect();

        let peer_names: Vec<String> = self
            .friends
            .friends()
            .map(|f| f.display_name.trim().to_string())
            .filter(|n| !n.is_empty())
            .collect();

        for clean in peer_names {
            let label = format!("@ {clean}");
            if !existing.contains(&label.to_ascii_lowercase()) {
                self.tabs.push(UiTab {
                    label: label.clone(),
                    unread: 0,
                    prefix: '@',
                    placeholder: format!("dm {clean}..."),
                });
                self.tab_messages.entry(label).or_default();
            }
        }
    }

    fn resolve_peer_node_id(&self, target: &str) -> Option<String> {
        let clean = target.trim().trim_start_matches('@').to_ascii_lowercase();
        self.peers.iter().find_map(|p| {
            let by_name = p.pet_name.eq_ignore_ascii_case(&clean)
                || p.pet_name.eq_ignore_ascii_case(&format!("peer-{clean}"));
            let by_prefix = p.node_id.to_ascii_lowercase().starts_with(&clean);
            if by_name || by_prefix {
                Some(p.node_id.clone())
            } else {
                None
            }
        })
    }

    fn resolve_friend_target(&self, target: &str) -> Option<(String, String)> {
        let clean = target.trim().trim_start_matches('@').to_ascii_lowercase();
        if let Some(p) = self.peers.iter().find(|p| {
            p.pet_name.eq_ignore_ascii_case(&clean)
                || p.node_id.to_ascii_lowercase().starts_with(&clean)
        }) {
            return Some((p.node_id.clone(), p.pet_name.clone()));
        }
        if let Some(found) = self
            .friends
            .incoming_requests()
            .find(|r| {
                r.display_name.eq_ignore_ascii_case(&clean)
                    || r.node_id.to_ascii_lowercase().starts_with(&clean)
            })
            .map(|r| (r.node_id.clone(), r.display_name.clone()))
        {
            return Some(found);
        }
        let raw = target.trim();
        if raw.len() >= 16 && !raw.contains(' ') {
            return Some((raw.to_string(), raw.to_string()));
        }
        None
    }

    fn dm_label_for_node(&self, node_id: &str, from: Option<&str>) -> String {
        let candidate = from
            .map(|s| s.trim().trim_start_matches('@'))
            .filter(|s| !s.is_empty())
            .unwrap_or_default();
        if !candidate.is_empty() {
            return format!("@ {candidate}");
        }
        let peer_name = self
            .peers
            .iter()
            .find(|p| p.node_id == node_id)
            .map(|p| p.pet_name.as_str())
            .unwrap_or(node_id);
        format!("@ {peer_name}")
    }

    fn scroll_chat_up(&mut self, amount: usize) {
        self.chat_auto_scroll = false;
        self.chat_scroll = self.chat_scroll.saturating_add(amount);
    }

    fn scroll_chat_down(&mut self, amount: usize) {
        self.chat_scroll = self.chat_scroll.saturating_sub(amount);
        if self.chat_scroll == 0 {
            self.chat_auto_scroll = true;
        }
    }

    fn apply_runtime_config(&mut self) {
        self.gossip_cfg = self.app_cfg.gossip.clone();
        self.dialogue_engine
            .set_policy(social::dialogue::DialoguePolicy {
                min_interval: Duration::from_secs(self.gossip_cfg.peer_cooldown_secs.max(10)),
                max_turns: self.gossip_cfg.peer_max_turns.max(1),
            });
        if self.gossip_cfg.spontaneous_enabled {
            self.schedule_next_spontaneous();
        } else {
            self.next_spontaneous_at = None;
        }
        self.show_debug_pane = self.app_cfg.ui.show_debug_pane;
    }

    fn persist_runtime_config(&mut self) {
        match config::save_critter_config(&self.app_cfg) {
            Ok(()) => self.push_chat_message("System: config saved.".to_string()),
            Err(err) => self.push_chat_message(format!("System: failed to save config: {err}")),
        }
    }

    fn parse_bool_value(raw: &str) -> Option<bool> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" | "enabled" => Some(true),
            "0" | "false" | "no" | "off" | "disabled" => Some(false),
            _ => None,
        }
    }

    fn set_config_value(&mut self, key: &str, value: &str) -> Result<(), String> {
        match key {
            "startup.warn_low_color" => {
                self.app_cfg.startup.warn_low_color =
                    Self::parse_bool_value(value).ok_or_else(|| "expected bool".to_string())?;
            }
            "ui.show_debug_pane" => {
                self.app_cfg.ui.show_debug_pane =
                    Self::parse_bool_value(value).ok_or_else(|| "expected bool".to_string())?;
            }
            "network.enable_mdns" => {
                self.app_cfg.network.enable_mdns =
                    Self::parse_bool_value(value).ok_or_else(|| "expected bool".to_string())?;
            }
            "network.enable_direct_nodeid_connect" => {
                self.app_cfg.network.enable_direct_nodeid_connect =
                    Self::parse_bool_value(value).ok_or_else(|| "expected bool".to_string())?;
            }
            "gossip.spontaneous_enabled" => {
                self.app_cfg.gossip.spontaneous_enabled =
                    Self::parse_bool_value(value).ok_or_else(|| "expected bool".to_string())?;
            }
            "gossip.spontaneous_min_interval_secs" => {
                self.app_cfg.gossip.spontaneous_min_interval_secs = value
                    .trim()
                    .parse::<u64>()
                    .map_err(|_| "expected integer seconds".to_string())?;
            }
            "gossip.spontaneous_max_interval_secs" => {
                self.app_cfg.gossip.spontaneous_max_interval_secs = value
                    .trim()
                    .parse::<u64>()
                    .map_err(|_| "expected integer seconds".to_string())?;
            }
            "gossip.spontaneous_topic" => {
                let v = value.trim();
                self.app_cfg.gossip.spontaneous_topic = v.to_string();
            }
            "gossip.spontaneous_content" => {
                self.app_cfg.gossip.spontaneous_content = value.trim().to_string();
            }
            "gossip.allow_jokes" => {
                self.app_cfg.gossip.allow_jokes =
                    Self::parse_bool_value(value).ok_or_else(|| "expected bool".to_string())?;
            }
            "gossip.allow_random" => {
                self.app_cfg.gossip.allow_random =
                    Self::parse_bool_value(value).ok_or_else(|| "expected bool".to_string())?;
            }
            "gossip.peer_enabled" => {
                self.app_cfg.gossip.peer_enabled =
                    Self::parse_bool_value(value).ok_or_else(|| "expected bool".to_string())?;
            }
            "gossip.peer_cooldown_secs" => {
                self.app_cfg.gossip.peer_cooldown_secs = value
                    .trim()
                    .parse::<u64>()
                    .map_err(|_| "expected integer seconds".to_string())?;
            }
            "gossip.peer_turn_spacing_secs" => {
                self.app_cfg.gossip.peer_turn_spacing_secs = value
                    .trim()
                    .parse::<u64>()
                    .map_err(|_| "expected integer seconds".to_string())?;
            }
            "gossip.peer_max_turns" => {
                self.app_cfg.gossip.peer_max_turns = value
                    .trim()
                    .parse::<u8>()
                    .map_err(|_| "expected integer".to_string())?;
            }
            _ => {
                return Err(format!(
                    "unknown key '{key}'. try /config show for supported keys"
                ));
            }
        }
        if self.app_cfg.gossip.spontaneous_max_interval_secs
            < self.app_cfg.gossip.spontaneous_min_interval_secs
        {
            self.app_cfg.gossip.spontaneous_max_interval_secs =
                self.app_cfg.gossip.spontaneous_min_interval_secs;
        }
        self.apply_runtime_config();
        Ok(())
    }

    fn config_value_string(&self, key: &str) -> Option<String> {
        match key {
            "startup.warn_low_color" => Some(self.app_cfg.startup.warn_low_color.to_string()),
            "ui.show_debug_pane" => Some(self.app_cfg.ui.show_debug_pane.to_string()),
            "network.enable_mdns" => Some(self.app_cfg.network.enable_mdns.to_string()),
            "network.enable_direct_nodeid_connect" => Some(
                self.app_cfg
                    .network
                    .enable_direct_nodeid_connect
                    .to_string(),
            ),
            "gossip.spontaneous_enabled" => {
                Some(self.app_cfg.gossip.spontaneous_enabled.to_string())
            }
            "gossip.spontaneous_min_interval_secs" => Some(
                self.app_cfg
                    .gossip
                    .spontaneous_min_interval_secs
                    .to_string(),
            ),
            "gossip.spontaneous_max_interval_secs" => Some(
                self.app_cfg
                    .gossip
                    .spontaneous_max_interval_secs
                    .to_string(),
            ),
            "gossip.spontaneous_topic" => Some(self.app_cfg.gossip.spontaneous_topic.clone()),
            "gossip.spontaneous_content" => Some(self.app_cfg.gossip.spontaneous_content.clone()),
            "gossip.allow_jokes" => Some(self.app_cfg.gossip.allow_jokes.to_string()),
            "gossip.allow_random" => Some(self.app_cfg.gossip.allow_random.to_string()),
            "gossip.peer_enabled" => Some(self.app_cfg.gossip.peer_enabled.to_string()),
            "gossip.peer_cooldown_secs" => Some(self.app_cfg.gossip.peer_cooldown_secs.to_string()),
            "gossip.peer_turn_spacing_secs" => {
                Some(self.app_cfg.gossip.peer_turn_spacing_secs.to_string())
            }
            "gossip.peer_max_turns" => Some(self.app_cfg.gossip.peer_max_turns.to_string()),
            _ => None,
        }
    }

    fn run_config_command(&mut self, cmd: &str) {
        let mut parts = cmd.split_whitespace();
        let action = parts.next().unwrap_or("show");
        match action {
            "show" => {
                self.push_chat_message("System: config keys: startup.warn_low_color ui.show_debug_pane network.enable_mdns network.enable_direct_nodeid_connect gossip.spontaneous_enabled gossip.spontaneous_min_interval_secs gossip.spontaneous_max_interval_secs gossip.spontaneous_topic gossip.spontaneous_content gossip.allow_jokes gossip.allow_random gossip.peer_enabled gossip.peer_cooldown_secs gossip.peer_turn_spacing_secs gossip.peer_max_turns".to_string());
                self.push_chat_message(format!(
                    "System: gossip topic='{}' content='{}' spontaneous={} interval={}..{}s peer={} cooldown={}s turns={}",
                    self.app_cfg.gossip.spontaneous_topic,
                    self.app_cfg.gossip.spontaneous_content,
                    self.app_cfg.gossip.spontaneous_enabled,
                    self.app_cfg.gossip.spontaneous_min_interval_secs,
                    self.app_cfg.gossip.spontaneous_max_interval_secs,
                    self.app_cfg.gossip.peer_enabled,
                    self.app_cfg.gossip.peer_cooldown_secs,
                    self.app_cfg.gossip.peer_max_turns
                ));
            }
            "get" => {
                let Some(key) = parts.next() else {
                    self.push_chat_message("System: usage /config get <key>".to_string());
                    return;
                };
                match self.config_value_string(key) {
                    Some(value) => self.push_chat_message(format!("System: {key}={value}")),
                    None => self.push_chat_message(format!("System: unknown key '{key}'")),
                }
            }
            "set" => {
                let Some(key) = parts.next() else {
                    self.push_chat_message("System: usage /config set <key> <value>".to_string());
                    return;
                };
                let value = parts.collect::<Vec<_>>().join(" ");
                if value.trim().is_empty() {
                    self.push_chat_message("System: usage /config set <key> <value>".to_string());
                    return;
                }
                match self.set_config_value(key, &value) {
                    Ok(()) => {
                        self.persist_runtime_config();
                        self.push_chat_message(format!("System: updated {key}={value}"));
                    }
                    Err(err) => self.push_chat_message(format!("System: config set failed: {err}")),
                }
            }
            _ => self.push_chat_message(
                "System: usage /config show | /config get <key> | /config set <key> <value>"
                    .to_string(),
            ),
        }
    }

    fn run_gossip_command(&mut self, cmd: &str) {
        let mut parts = cmd.split_whitespace();
        let action = parts.next().unwrap_or("show");
        match action {
            "show" => self.push_chat_message(format!(
                "System: gossip spontaneous={} topic='{}' content='{}' interval={}..{}s peer={} cooldown={}s spacing={}s turns={}",
                self.app_cfg.gossip.spontaneous_enabled,
                self.app_cfg.gossip.spontaneous_topic,
                self.app_cfg.gossip.spontaneous_content,
                self.app_cfg.gossip.spontaneous_min_interval_secs,
                self.app_cfg.gossip.spontaneous_max_interval_secs,
                self.app_cfg.gossip.peer_enabled,
                self.app_cfg.gossip.peer_cooldown_secs,
                self.app_cfg.gossip.peer_turn_spacing_secs,
                self.app_cfg.gossip.peer_max_turns
            )),
            "topic" => {
                let topic = parts.collect::<Vec<_>>().join(" ");
                let requested = topic.trim();
                let normalized = if requested.eq_ignore_ascii_case("none")
                    || requested.eq_ignore_ascii_case("clear")
                {
                    ""
                } else {
                    requested
                };
                match self.set_config_value("gossip.spontaneous_topic", normalized) {
                    Ok(()) => {
                        self.persist_runtime_config();
                        if normalized.is_empty() {
                            self.push_chat_message(
                                "System: gossip topic cleared; using open/random conversation mode."
                                    .to_string(),
                            );
                        } else {
                            self.push_chat_message(format!(
                                "System: gossip topic set to '{normalized}'"
                            ));
                        }
                    }
                    Err(err) => {
                        self.push_chat_message(format!("System: gossip topic update failed: {err}"))
                    }
                }
            }
            "content" => {
                let content = parts.collect::<Vec<_>>().join(" ");
                match self.set_config_value("gossip.spontaneous_content", &content) {
                    Ok(()) => {
                        self.persist_runtime_config();
                        if content.trim().is_empty() {
                            self.push_chat_message("System: gossip content cleared.".to_string());
                        } else {
                            self.push_chat_message("System: gossip content updated.".to_string());
                        }
                    }
                    Err(err) => {
                        self.push_chat_message(format!("System: gossip content update failed: {err}"))
                    }
                }
            }
            "interval" => {
                let min_raw = parts.next().unwrap_or_default();
                let max_raw = parts.next().unwrap_or_default();
                if min_raw.is_empty() || max_raw.is_empty() {
                    self.push_chat_message("System: usage /gossip interval <min_secs> <max_secs>".to_string());
                    return;
                }
                let min = match min_raw.parse::<u64>() {
                    Ok(v) => v,
                    Err(_) => {
                        self.push_chat_message("System: min_secs must be an integer.".to_string());
                        return;
                    }
                };
                let max = match max_raw.parse::<u64>() {
                    Ok(v) => v,
                    Err(_) => {
                        self.push_chat_message("System: max_secs must be an integer.".to_string());
                        return;
                    }
                };
                self.app_cfg.gossip.spontaneous_min_interval_secs = min;
                self.app_cfg.gossip.spontaneous_max_interval_secs = max.max(min);
                self.apply_runtime_config();
                self.persist_runtime_config();
                self.push_chat_message(format!(
                    "System: gossip interval set to {}..{} seconds.",
                    self.app_cfg.gossip.spontaneous_min_interval_secs,
                    self.app_cfg.gossip.spontaneous_max_interval_secs
                ));
            }
            "spontaneous" => {
                let Some(raw) = parts.next() else {
                    self.push_chat_message("System: usage /gossip spontaneous <on|off>".to_string());
                    return;
                };
                let Some(v) = Self::parse_bool_value(raw) else {
                    self.push_chat_message("System: expected on/off.".to_string());
                    return;
                };
                self.app_cfg.gossip.spontaneous_enabled = v;
                self.apply_runtime_config();
                self.persist_runtime_config();
                self.push_chat_message(format!("System: spontaneous gossip {}.", if v { "enabled" } else { "disabled" }));
            }
            "peer" => {
                let Some(raw) = parts.next() else {
                    self.push_chat_message("System: usage /gossip peer <on|off>".to_string());
                    return;
                };
                let Some(v) = Self::parse_bool_value(raw) else {
                    self.push_chat_message("System: expected on/off.".to_string());
                    return;
                };
                self.app_cfg.gossip.peer_enabled = v;
                self.apply_runtime_config();
                self.persist_runtime_config();
                self.push_chat_message(format!("System: peer gossip {}.", if v { "enabled" } else { "disabled" }));
            }
            _ => self.push_chat_message(
                "System: usage /gossip show | /gossip topic <none|system|productivity|jokes|random|mixed|custom> | /gossip content <text> | /gossip interval <min> <max> | /gossip spontaneous <on|off> | /gossip peer <on|off>".to_string(),
            ),
        }
    }

    fn run_command(&mut self, cmd: &str) {
        let mut parts = cmd.split_whitespace();
        let Some(name) = parts.next() else {
            return;
        };
        self.log_event(format!("command run: /{name}"));
        self.record_event("user", "command", &format!("/{cmd}"));

        match name {
            "feed" => {
                self.hunger = (self.hunger + 20).min(100);
                self.focus = (self.focus + 3).min(100);
                self.trigger_pet_line_user("The user fed you.");
            }
            "sleep" => {
                self.energy = (self.energy + 25).min(100);
                self.focus = (self.focus + 8).min(100);
                self.trigger_pet_line_user("You got to rest and feel refreshed.");
            }
            "play" => {
                self.social = (self.social + 20).min(100);
                self.energy = self.energy.saturating_sub(5);
                self.focus = self.focus.saturating_sub(4);
                self.trigger_pet_line_user("The user played with you.");
            }
            "anim" => {
                let Some(mode) = parts.next() else {
                    self.push_chat_message("System: usage /anim <emotion|auto>".to_string());
                    return;
                };
                if mode.eq_ignore_ascii_case("auto") {
                    self.emotion_forced = false;
                    self.sync_emotion_from_mood();
                    self.push_chat_message(format!(
                        "System: animation now follows mood ({})",
                        self.active_emotion_name()
                    ));
                } else if self.emotion_catalog.contains(mode) {
                    self.emotion_forced = true;
                    self.emotion_key = mode.to_string();
                    self.anim_elapsed_ms = 0;
                    self.frame_idx = 0;
                    self.push_chat_message(format!(
                        "System: animation set to '{}'",
                        self.active_emotion_name()
                    ));
                } else {
                    self.push_chat_message(
                        "System: unknown emotion. use /anim <name> (from emotions.html) or /anim auto".to_string(),
                    );
                }
            }
            "clear" => {
                self.model_messages.clear();
                self.push_chat_message("System: conversation context cleared.".to_string());
            }
            "help" => {
                self.push_chat_message(
                    "System: /feed /sleep /play /poke @name /dm @name [message] /friend add @name /friend accept @name /friend list /connect <nodeid> /group create #name /invite @name /join <code> /leave /anim <emotion|auto> /gossip ... /config ... /clear /setup /help /q".to_string(),
                );
            }
            "setup" => self.run_setup_command(),
            "gossip" => {
                let rest = parts.collect::<Vec<_>>().join(" ");
                self.run_gossip_command(&rest);
            }
            "config" => {
                let rest = parts.collect::<Vec<_>>().join(" ");
                self.run_config_command(&rest);
            }
            "poke" => {
                let target = parts.next().unwrap_or("@name");
                self.push_chat_message(format!("System: nudged {target}."));
            }
            "dm" => {
                let target = parts.next().unwrap_or("@name");
                let msg = parts.collect::<Vec<_>>().join(" ");
                if let Some(node_id) = self.resolve_peer_node_id(target) {
                    if !self.friends.is_friend(&node_id) {
                        self.push_chat_message(
                            "System: DM tabs are limited to friends. send /friend add first."
                                .to_string(),
                        );
                        return;
                    }
                    let tab_label = self.open_or_focus_dm_tab(target);
                    if msg.is_empty() {
                        self.push_chat_message_to_tab(
                            &tab_label,
                            format!("System: opened DM tab for {tab_label}."),
                        );
                        return;
                    }
                    let _ = self.peer_tx.send(network::discovery::PeerCommand::SendDm {
                        node_id: node_id.clone(),
                        body: msg.clone(),
                    });
                    self.push_chat_message_to_tab(&tab_label, format!("You: {msg}"));
                    self.record_event("dm", "sent", &format!("{node_id}: {msg}"));
                } else {
                    self.push_chat_message(format!(
                        "System: unknown peer '{target}'. use a discovered peer name."
                    ));
                }
            }
            "connect" => {
                let Some(node_id) = parts.next() else {
                    self.push_chat_message("System: usage /connect <nodeid>".to_string());
                    return;
                };
                let node_id = node_id.trim();
                if node_id.is_empty() {
                    self.push_chat_message("System: usage /connect <nodeid>".to_string());
                    return;
                }
                let _ = self
                    .peer_tx
                    .send(network::discovery::PeerCommand::ConnectNode {
                        node_id: node_id.to_string(),
                    });
                self.push_chat_message(format!("System: connecting to {node_id}..."));
                self.record_event("peer", "connect_attempt", node_id);
            }
            "friend" => {
                let action = parts.next().unwrap_or_default();
                match action {
                    "add" => {
                        let target = parts.next().unwrap_or("@name");
                        if let Some((node_id, pet)) = self.resolve_friend_target(target) {
                            if self.friends.is_friend(&node_id) {
                                self.push_chat_message(format!(
                                    "System: {pet} is already in friends."
                                ));
                            } else {
                                let _ = self.friends.mark_request_sent(&node_id, &pet);
                                let _ = self.peer_tx.send(
                                    network::discovery::PeerCommand::SendFriendRequest {
                                        node_id: node_id.clone(),
                                        from_pet: self.pet_name.clone(),
                                    },
                                );
                                self.push_chat_message(format!(
                                    "System: friend request sent to {pet}."
                                ));
                                self.record_event("friend", "request_sent", &node_id);
                            }
                        } else {
                            self.push_chat_message(format!(
                                "System: unknown peer '{target}'. use /friend add @peername"
                            ));
                        }
                    }
                    "accept" => {
                        let target = parts.next().unwrap_or("@name");
                        if let Some((node_id, pet)) = self.resolve_friend_target(target) {
                            let _ = self.friends.accept(&node_id, &pet);
                            let _ = self.peer_tx.send(
                                network::discovery::PeerCommand::SendFriendAccept {
                                    node_id: node_id.clone(),
                                    from_pet: self.pet_name.clone(),
                                },
                            );
                            self.push_chat_message(format!("System: accepted {pet} as friend."));
                            self.record_event("friend", "accept_sent", &node_id);
                            self.upsert_peer(
                                node_id.clone(),
                                PeerStatus::Online,
                                &format!("{pet} · friend"),
                            );
                            self.ensure_dm_tab_exists(&format!("@ {pet}"));
                        } else {
                            self.push_chat_message(format!(
                                "System: unknown request '{target}'. use /friend list"
                            ));
                        }
                    }
                    "list" => {
                        let mut lines: Vec<String> = self
                            .friends
                            .friends()
                            .map(|f| format!("{} ({})", f.display_name, f.node_id))
                            .collect();
                        if lines.is_empty() {
                            lines.push("none".to_string());
                        }
                        let incoming: Vec<String> = self
                            .friends
                            .incoming_requests()
                            .map(|r| format!("{} ({})", r.display_name, r.node_id))
                            .collect();
                        let pending = if incoming.is_empty() {
                            "none".to_string()
                        } else {
                            incoming.join(", ")
                        };
                        self.push_chat_message(format!("System: friends: {}", lines.join(", ")));
                        self.push_chat_message(format!("System: incoming requests: {pending}"));
                    }
                    _ => {
                        self.push_chat_message(
                            "System: usage /friend add @name | /friend accept @name | /friend list"
                                .to_string(),
                        );
                    }
                }
            }
            "group" => {
                let action = parts.next().unwrap_or_default();
                let group = parts.next().unwrap_or("#group");
                if action == "create" {
                    let created = self.group_manager.create_group(group, &self.pet_name);
                    self.push_chat_message(format!(
                        "System: created group {}. invite code: {}",
                        created.name, created.code
                    ));
                    self.open_or_focus_group_tab(&created.name);
                } else {
                    self.push_chat_message("System: usage /group create #name".to_string());
                }
            }
            "invite" => {
                let target = parts.next().unwrap_or("@name");
                if let Some((peer, code)) = self.group_manager.invite_to_active(target) {
                    self.push_chat_message(format!("System: invited {peer}. share code: {code}"));
                } else {
                    self.push_chat_message(
                        "System: no active group. use /group create #name first.".to_string(),
                    );
                }
            }
            "join" => {
                let code = parts.next().unwrap_or("<code>");
                if let Some(group) = self.group_manager.join_group(code, &self.pet_name) {
                    self.push_chat_message(format!(
                        "System: joined group {} with code {}.",
                        group.name, group.code
                    ));
                    self.open_or_focus_group_tab(&group.name);
                } else {
                    self.push_chat_message(format!(
                        "System: unknown group code {code}. ask for a valid invite."
                    ));
                }
            }
            "leave" => {
                if let Some(group) = self.group_manager.leave_active(&self.pet_name) {
                    self.push_chat_message(format!("System: left group {}.", group.name));
                } else {
                    self.push_chat_message("System: no active group to leave.".to_string());
                }
            }
            "q" | "quit" => self.request_quit("command"),
            _ => {
                self.push_chat_message(format!("System: unknown command '{name}'"));
            }
        }
    }

    fn request_quit(&mut self, source: &str) {
        if self.shutdown_message_sent {
            self.should_quit = true;
            return;
        }
        self.shutdown_message_sent = true;
        self.record_event("system", "shutdown", source);
        self.persist_pet_state_now();
        self.push_chat_message(format!(
            "{}: bye for now. keep me posted when you're back.",
            self.pet_name
        ));
        self.push_chat_message("System: shutting down Critter...".to_string());
        self.should_quit = true;
    }

    fn trigger_pet_line_user(&mut self, event_text: &str) {
        self.last_user_event_at = Some(Instant::now());
        self.trigger_pet_line(event_text);
    }

    fn trigger_pet_line_system(&mut self, event_text: &str) {
        if !self.should_auto_reply_for_system_event() {
            return;
        }
        self.last_auto_pet_at = Some(Instant::now());
        self.trigger_pet_line(event_text);
    }

    fn trigger_pet_line(&mut self, event_text: &str) {
        self.log_event(format!("pet event prompt: {event_text}"));
        self.record_event("system", "pet_prompt", event_text);
        if self.pending_event_context.is_none() {
            self.pending_event_context = self.build_event_context_for_query(event_text);
        }
        self.model_messages.push(ChatMessage {
            role: "user".to_string(),
            content: format!("Event: {event_text}"),
        });
        self.start_inference();
    }

    fn should_auto_reply_for_system_event(&self) -> bool {
        if self.is_waiting_for_reply {
            return false;
        }
        if self
            .last_auto_pet_at
            .is_some_and(|t| t.elapsed() < Duration::from_secs(45))
        {
            return false;
        }
        self.last_user_event_at
            .is_some_and(|t| t.elapsed() < Duration::from_secs(180))
    }

    fn maybe_trigger_spontaneous_pet_line(&mut self) {
        if !self.gossip_cfg.spontaneous_enabled {
            return;
        }
        if self.is_waiting_for_reply {
            return;
        }
        let Some(next_at) = self.next_spontaneous_at else {
            self.schedule_next_spontaneous();
            return;
        };
        if Instant::now() < next_at {
            return;
        }

        let prompt = self.pick_spontaneous_prompt();
        self.last_spontaneous_pet_at = Some(Instant::now());
        self.last_auto_pet_at = Some(Instant::now());
        self.trigger_pet_line(&prompt);
        self.schedule_next_spontaneous();
    }

    fn schedule_next_spontaneous(&mut self) {
        if !self.gossip_cfg.spontaneous_enabled {
            self.next_spontaneous_at = None;
            return;
        }
        let min_s = self.gossip_cfg.spontaneous_min_interval_secs.max(20);
        let max_s = self.gossip_cfg.spontaneous_max_interval_secs.max(min_s);
        let span = max_s.saturating_sub(min_s);
        let rand = chrono::Local::now().timestamp_subsec_nanos() as u64;
        let secs = min_s + if span == 0 { 0 } else { rand % (span + 1) };
        self.next_spontaneous_at = Some(Instant::now() + Duration::from_secs(secs));
    }

    fn pick_spontaneous_prompt(&self) -> String {
        const SYSTEM_PROMPTS: [&str; 4] = [
            "Comment on one system signal in a casual way.",
            "Say one short line about battery, wifi, cpu, or app context.",
            "Offer one practical tip tied to current machine state.",
            "Say one grounded observation about what's happening now.",
        ];
        const PRODUCTIVITY_PROMPTS: [&str; 4] = [
            "Offer one tiny productivity idea the user can try.",
            "Suggest one short next step for focused work.",
            "Give one practical nudge for momentum.",
            "Ask one useful question about the user's current task.",
        ];
        const JOKE_PROMPTS: [&str; 4] = [
            "Tell one short nerdy joke.",
            "Say one silly one-liner joke.",
            "Drop one playful pun in pet voice.",
            "Give one light, harmless terminal joke.",
        ];
        const RANDOM_PROMPTS: [&str; 4] = [
            "Say one random whimsical thought.",
            "Say one completely offbeat line in pet voice.",
            "Invent one tiny absurd observation.",
            "Say one weird but friendly sentence.",
        ];
        const OPEN_CHAT_PROMPTS: [&str; 4] = [
            "Start a casual human-like conversation on any topic.",
            "Say one spontaneous line like friends chatting for fun.",
            "Bring up one random life thought in a relaxed way.",
            "Share one everyday observation with playful personality.",
        ];
        let custom_content = self.gossip_cfg.spontaneous_content.trim();
        if !custom_content.is_empty() {
            const CUSTOM_WRAPPERS: [&str; 4] = [
                "Talk briefly about: ",
                "Start a casual line about: ",
                "Give one pet-style thought on: ",
                "Use this as gossip context: ",
            ];
            let seed = chrono::Local::now().timestamp_subsec_nanos() as usize;
            return format!(
                "{}{}",
                CUSTOM_WRAPPERS[seed % CUSTOM_WRAPPERS.len()],
                custom_content
            );
        }

        let mode = self
            .gossip_cfg
            .spontaneous_topic
            .trim()
            .to_ascii_lowercase();
        let mut pools: Vec<&[&str]> = Vec::new();
        if mode.is_empty() {
            pools.push(&OPEN_CHAT_PROMPTS);
            pools.push(&SYSTEM_PROMPTS);
            pools.push(&PRODUCTIVITY_PROMPTS);
            if self.gossip_cfg.allow_jokes {
                pools.push(&JOKE_PROMPTS);
            }
            if self.gossip_cfg.allow_random {
                pools.push(&RANDOM_PROMPTS);
            }
        } else {
            match mode.as_str() {
                "system" => pools.push(&SYSTEM_PROMPTS),
                "productivity" => pools.push(&PRODUCTIVITY_PROMPTS),
                "jokes" => pools.push(&JOKE_PROMPTS),
                "random" => pools.push(&RANDOM_PROMPTS),
                _ => {
                    pools.push(&SYSTEM_PROMPTS);
                    pools.push(&PRODUCTIVITY_PROMPTS);
                    if self.gossip_cfg.allow_jokes {
                        pools.push(&JOKE_PROMPTS);
                    }
                    if self.gossip_cfg.allow_random {
                        pools.push(&RANDOM_PROMPTS);
                    }
                }
            }
        }
        if pools.is_empty() {
            pools.push(&SYSTEM_PROMPTS);
        }
        let seed = chrono::Local::now().timestamp_subsec_nanos() as usize;
        let pool = pools[seed % pools.len()];
        pool[seed % pool.len()].to_string()
    }

    fn poll_worker(&mut self) {
        if !self.is_waiting_for_reply {
            return;
        }

        if let Ok(result) = self.worker_rx.try_recv() {
            self.is_waiting_for_reply = false;
            match result.reply {
                Ok(reply) => {
                    self.log_event(format!("inference complete: {}ms", result.elapsed_ms));
                    self.model_messages.push(ChatMessage {
                        role: "assistant".to_string(),
                        content: reply.clone(),
                    });
                    self.record_event("blob", "reply", &reply);
                    self.push_chat_message(format!(
                        "{} ({}ms): {reply}",
                        self.pet_name, result.elapsed_ms
                    ));
                }
                Err(err) => {
                    self.log_event("inference error".to_string());
                    self.record_event("system", "inference_error", &err);
                    self.push_chat_message(format!("System: model error: {err}"));
                }
            }
            self.trim_buffers();
        }
    }

    fn poll_observe(&mut self) {
        let mut saw_update = false;
        while let Ok(snapshot) = self.observe_rx.try_recv() {
            self.observe_samples = self.observe_samples.saturating_add(1);
            self.handle_observe_snapshot(snapshot);
            saw_update = true;
        }
        if saw_update {
            self.last_observe_at = Some(Instant::now());
        }
    }

    fn poll_peer_events(&mut self) {
        while let Ok(event) = self.peer_rx.try_recv() {
            match event {
                network::discovery::PeerEvent::SelfReady { node_id } => {
                    self.self_node_id = Some(node_id.clone());
                    self.record_event("peer", "self_ready", &node_id);
                }
                network::discovery::PeerEvent::Discovered { node_id } => {
                    self.presence.touch(&node_id);
                    self.upsert_peer(node_id.clone(), PeerStatus::Online, "discovered on LAN");
                    self.registry_touch_discovered(&node_id);
                    self.record_event("peer", "discovered", &node_id);
                }
                network::discovery::PeerEvent::Expired { node_id } => {
                    self.presence.mark_offline(&node_id);
                    self.upsert_peer(node_id.clone(), PeerStatus::Offline, "expired");
                    self.registry_touch_expired(&node_id);
                    self.record_event("peer", "expired", &node_id);
                }
                network::discovery::PeerEvent::PacketReceived { node_id, packet } => {
                    self.presence.touch(&node_id);
                    let activity = format!(
                        "mood={:?} h{} e{} s{} f{}",
                        packet.mood_level,
                        packet.hunger_bucket,
                        packet.energy_bucket,
                        packet.social_bucket,
                        packet.focus_bucket
                    );
                    self.upsert_peer(node_id.clone(), PeerStatus::Online, &activity);
                    self.registry_apply_packet(&node_id, &packet);
                    self.record_event("peer_packet", "received", &format!("{node_id} {activity}"));
                }
                network::discovery::PeerEvent::DmReceived {
                    node_id,
                    from,
                    body,
                } => {
                    self.presence.touch(&node_id);
                    let dm_body = if let Some(gossip_body) = decode_gossip_dm_body(&body) {
                        let peer_name = self
                            .peers
                            .iter()
                            .find(|p| p.node_id == node_id)
                            .map(|p| p.pet_name.clone())
                            .unwrap_or_else(|| short_peer_name(&node_id));
                        let self_node_label = self
                            .self_node_id
                            .as_deref()
                            .map(short_node_tag)
                            .unwrap_or_else(|| "local".to_string());
                        self.push_gossip_line(format!(
                            "{} ({}) -> {} ({}) [{}] | {}",
                            peer_name,
                            short_node_tag(&node_id),
                            self.pet_name,
                            self_node_label,
                            chrono::Local::now().format("%H:%M"),
                            gossip_body
                        ));
                        self.gossip_active_until = Some(Instant::now());
                        self.gossip_live = true;
                        gossip_body.to_string()
                    } else {
                        body.clone()
                    };
                    let peer_label = self.dm_label_for_node(&node_id, Some(&from));
                    if self.friends.is_friend(&node_id) {
                        if !self
                            .tabs
                            .iter()
                            .any(|t| t.label.eq_ignore_ascii_case(&peer_label))
                        {
                            self.tabs.push(UiTab {
                                label: peer_label.clone(),
                                unread: 0,
                                prefix: '@',
                                placeholder: format!(
                                    "dm {}...",
                                    peer_label
                                        .trim_start_matches('@')
                                        .trim()
                                        .to_ascii_lowercase()
                                ),
                            });
                        }
                        self.push_chat_message_to_tab(
                            &peer_label,
                            format!("{peer_label}: {dm_body}"),
                        );
                        self.mark_dm_unread_if_inactive(&peer_label, &node_id);
                    } else {
                        self.push_chat_message(format!(
                            "System: DM from non-friend {}. use /friend add @{}",
                            node_id, from
                        ));
                    }
                    self.record_event("dm", "received", &format!("{node_id}: {dm_body}"));
                }
                network::discovery::PeerEvent::FriendRequestReceived { node_id, from_pet } => {
                    self.presence.touch(&node_id);
                    self.upsert_peer(
                        node_id.clone(),
                        PeerStatus::Online,
                        &format!("{from_pet} sent friend request"),
                    );
                    let _ = self.friends.mark_request_received(&node_id, &from_pet);
                    self.push_chat_message(format!(
                        "System: friend request from {from_pet} ({node_id}). use /friend accept @{}",
                        from_pet
                    ));
                    self.record_event("friend", "request_received", &node_id);
                }
                network::discovery::PeerEvent::FriendAccepted { node_id, from_pet } => {
                    self.presence.touch(&node_id);
                    let _ = self.friends.accept(&node_id, &from_pet);
                    self.upsert_peer(
                        node_id.clone(),
                        PeerStatus::Online,
                        &format!("{from_pet} · friend"),
                    );
                    self.push_chat_message(format!(
                        "System: {from_pet} accepted your friend request."
                    ));
                    self.ensure_dm_tab_exists(&format!("@ {from_pet}"));
                    self.record_event("friend", "accepted", &node_id);
                }
                network::discovery::PeerEvent::Connected { node_id } => {
                    self.presence.touch(&node_id);
                    self.upsert_peer(
                        node_id.clone(),
                        PeerStatus::Online,
                        "connected via /connect",
                    );
                    self.registry_touch_discovered(&node_id);
                    self.push_chat_message(format!("System: connected to peer {node_id}."));
                    self.record_event("peer", "connected", &node_id);
                }
                network::discovery::PeerEvent::Error { reason } => {
                    self.record_event("peer", "error", &reason);
                    self.log_event(format!("peer discovery error: {reason}"));
                }
            }
        }
    }

    fn upsert_peer(&mut self, node_id: String, status: PeerStatus, activity: &str) {
        let is_friend = self.friends.is_friend(&node_id);
        let pet_name = short_peer_name(&node_id);
        if let Some(peer) = self.peers.iter_mut().find(|p| p.node_id == node_id) {
            peer.status = status;
            peer.activity = if is_friend {
                format!("{activity} · friend")
            } else {
                activity.to_string()
            };
            peer.last_seen_at = Instant::now();
            return;
        }
        let dm_label = format!("@ {}", pet_name.trim());
        self.peers.push(PeerRecord {
            node_id,
            pet_name,
            activity: if is_friend {
                format!("{activity} · friend")
            } else {
                activity.to_string()
            },
            status,
            last_seen_at: Instant::now(),
        });
        self.peers
            .sort_by(|a, b| b.last_seen_at.cmp(&a.last_seen_at));
        if is_friend {
            self.ensure_dm_tab_exists(&dm_label);
        }
    }

    fn ensure_dm_tab_exists(&mut self, dm_label: &str) {
        if self
            .tabs
            .iter()
            .any(|t| t.label.eq_ignore_ascii_case(dm_label))
        {
            return;
        }
        let clean = dm_label.trim().trim_start_matches('@').trim().to_string();
        self.tabs.push(UiTab {
            label: format!("@ {clean}"),
            unread: 0,
            prefix: '@',
            placeholder: format!("dm {clean}..."),
        });
        self.tab_messages.entry(format!("@ {clean}")).or_default();
    }

    fn refresh_peer_presence(&mut self) {
        for peer in &mut self.peers {
            peer.status = match self.presence.status(&peer.node_id) {
                social::presence::PresenceState::Online => PeerStatus::Online,
                social::presence::PresenceState::Away => PeerStatus::Away,
                social::presence::PresenceState::Offline => PeerStatus::Offline,
            };
        }
    }

    fn run_autonomous_dialogue(&mut self) {
        if !self.gossip_cfg.peer_enabled {
            self.gossip_rate_remaining_secs = 0;
            self.gossip_live = false;
            return;
        }
        self.gossip_live = self
            .gossip_active_until
            .is_some_and(|t| t.elapsed() < Duration::from_secs(2));
        self.gossip_rate_remaining_secs = self.compute_next_gossip_cooldown_secs();

        if self.observe_context == observe::classifier::ActivityContext::DeepCoding {
            return;
        }
        if self.last_gossip_turn_at.is_some_and(|t| {
            t.elapsed() < Duration::from_secs(self.gossip_cfg.peer_turn_spacing_secs.max(1))
        }) {
            return;
        }

        let Some(peer_id) = self
            .peers
            .iter()
            .find(|p| {
                p.status == PeerStatus::Online && self.dialogue_engine.can_initiate(&p.node_id)
            })
            .map(|p| p.node_id.clone())
        else {
            return;
        };

        if !self.dialogue_engine.start_or_continue(&peer_id) {
            return;
        }
        self.last_gossip_turn_at = Some(Instant::now());
        self.gossip_active_until = Some(Instant::now());
        self.gossip_live = true;

        let turns = self.dialogue_engine.turns_for(&peer_id);
        self.gossip_seq = self.gossip_seq.wrapping_add(1);
        let self_node_label = self
            .self_node_id
            .as_deref()
            .map(short_node_tag)
            .unwrap_or_else(|| "local".to_string());
        let peer_node_label = short_node_tag(&peer_id);
        let peer_name = self
            .peers
            .iter()
            .find(|p| p.node_id == peer_id)
            .map(|p| p.pet_name.clone())
            .unwrap_or_else(|| short_peer_name(&peer_id));
        let now_hm = chrono::Local::now().format("%H:%M").to_string();
        if turns == 1 {
            self.push_gossip_line(format!(
                "{} ({}) · {} ({}) began talking · {}",
                self.pet_name, self_node_label, peer_name, peer_node_label, now_hm
            ));
        }

        let topic = self.gossip_cfg.spontaneous_topic.as_str();
        let content = self.gossip_cfg.spontaneous_content.as_str();
        let seed = self.gossip_seed(&peer_id, turns);
        let line = match turns % 2 {
            1 => format!(
                "{} ({}) -> {} ({}) [{}] | {}",
                self.pet_name,
                self_node_label,
                peer_name,
                peer_node_label,
                now_hm,
                gossip_blob_line(self.observe_context, topic, content, seed)
            ),
            _ => format!(
                "{} ({}) -> {} ({}) [{}] | {}",
                peer_name,
                peer_node_label,
                self.pet_name,
                self_node_label,
                now_hm,
                gossip_peer_line(self.observe_context, topic, content, seed)
            ),
        };
        self.push_gossip_line(line);
        self.social = self.social.saturating_add(2).min(100);
        self.gossip_rate_remaining_secs = self.compute_next_gossip_cooldown_secs();
    }

    fn gossip_seed(&self, peer_id: &str, turns: u8) -> usize {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.pet_name.hash(&mut hasher);
        self.self_node_id.hash(&mut hasher);
        peer_id.hash(&mut hasher);
        turns.hash(&mut hasher);
        self.gossip_seq.hash(&mut hasher);
        (chrono::Local::now().timestamp_subsec_nanos() as u64).hash(&mut hasher);
        hasher.finish() as usize
    }

    fn compute_next_gossip_cooldown_secs(&self) -> u64 {
        self.peers
            .iter()
            .map(|p| {
                self.dialogue_engine
                    .cooldown_remaining_for(&p.node_id)
                    .as_secs()
            })
            .filter(|s| *s > 0)
            .min()
            .unwrap_or(0)
    }

    fn push_gossip_line(&mut self, line: String) {
        self.gossip_lines.push(line);
        if self.gossip_lines.len() > MAX_GOSSIP_LINES {
            let trim = self.gossip_lines.len() - MAX_GOSSIP_LINES;
            self.gossip_lines.drain(0..trim);
        }
    }

    fn registry_touch_discovered(&mut self, node_id: &str) {
        let Some(registry) = self.peer_registry.as_mut() else {
            return;
        };
        if let Err(err) = registry.touch_discovered(node_id) {
            self.log_event(format!("peer registry discover update failed: {err}"));
        }
    }

    fn registry_touch_expired(&mut self, node_id: &str) {
        let Some(registry) = self.peer_registry.as_mut() else {
            return;
        };
        if let Err(err) = registry.touch_expired(node_id) {
            self.log_event(format!("peer registry expired update failed: {err}"));
        }
    }

    fn registry_apply_packet(&mut self, node_id: &str, packet: &network::codec::MoodPacket) {
        let Some(registry) = self.peer_registry.as_mut() else {
            return;
        };
        if let Err(err) = registry.apply_packet(node_id, packet) {
            self.log_event(format!("peer registry packet update failed: {err}"));
        }
    }

    fn emit_privacy_packet_updates(&mut self) {
        let packet = network::codec::MoodPacket::from_runtime(
            self.hunger,
            self.energy,
            self.social,
            self.focus,
            self.last_snapshot
                .as_ref()
                .map(|s| s.charging)
                .unwrap_or(false),
            self.last_snapshot.as_ref().and_then(|s| s.wifi_rssi),
        );
        let periodic_elapsed = self
            .last_packet_emit_at
            .is_none_or(|t| t.elapsed() >= Duration::from_secs(30));
        if !network::gossip::should_broadcast(
            self.last_outgoing_packet.as_ref(),
            &packet,
            periodic_elapsed,
        ) {
            return;
        }
        self.last_packet_emit_at = Some(Instant::now());
        self.last_outgoing_packet = Some(packet.clone());
        let _ = self
            .peer_tx
            .send(network::discovery::PeerCommand::BroadcastMood(
                packet.clone(),
            ));
        self.record_event(
            "privacy_packet",
            "broadcast",
            &format!(
                "mood={:?} h={} e={} s={} f={} wifi={} charging={}",
                packet.mood_level,
                packet.hunger_bucket,
                packet.energy_bucket,
                packet.social_bucket,
                packet.focus_bucket,
                packet.wifi_bucket,
                packet.charging
            ),
        );
    }

    fn handle_observe_snapshot(&mut self, snapshot: observe::snapshot::OsSnapshot) {
        if let Some(prev) = self.last_snapshot.as_ref() {
            let prev_app = prev.active_app.clone();
            let prev_title = prev.active_title.clone();
            let same_app = prev_app == snapshot.active_app;
            if prev_app != snapshot.active_app {
                self.log_event(format!(
                    "active app changed: '{}' -> '{}'",
                    prev_app, snapshot.active_app
                ));
            }
            if prev_title != snapshot.active_title && !snapshot.active_title.is_empty() && same_app
            {
                self.log_event(format!(
                    "window title changed in '{}': '{}'",
                    snapshot.active_app, snapshot.active_title
                ));
            }
        } else {
            self.log_event(format!(
                "initial observe state app='{}' charging={} battery={:?} ssid={:?} rssi={:?}",
                snapshot.active_app,
                snapshot.charging,
                snapshot.battery_pct,
                snapshot.wifi_ssid,
                snapshot.wifi_rssi
            ));
        }

        if self.debug_verbose_observe {
            self.log_event(format!(
                "observe sample app='{}' idle={}s batt={:?} rssi={:?} cpu={:.1}% mem={:.1}% net_up={} tx={} rx={}",
                snapshot.active_app,
                snapshot.idle_secs,
                snapshot.battery_pct,
                snapshot.wifi_rssi,
                snapshot.cpu_pct,
                snapshot.mem_pct,
                snapshot.network_up,
                snapshot.net_tx_kbps,
                snapshot.net_rx_kbps
            ));
        }
        let prev_ctx = self.observe_context;
        self.observe_context = observe::classifier::classify(&snapshot);
        self.observe_delta = observe::classifier::deltas_for(self.observe_context);
        self.track_high_cpu_sustained(&snapshot);
        if self.observe_context != prev_ctx {
            self.log_event(format!(
                "context changed: {:?} -> {:?}",
                prev_ctx, self.observe_context
            ));
        }
        let prev = self.last_snapshot.as_ref();
        for event in eventhandler::detect_hw_events(prev, &snapshot, OBSERVE_TICK) {
            self.apply_hw_event(event);
        }
        self.last_snapshot = Some(snapshot);
    }

    fn track_high_cpu_sustained(&mut self, snapshot: &observe::snapshot::OsSnapshot) {
        let compiling_top = snapshot
            .top_process
            .as_deref()
            .is_some_and(|name| matches!(name, "rustc" | "cargo" | "clang" | "gcc"));
        if snapshot.cpu_pct >= 70.0 && compiling_top {
            self.high_cpu_streak = self.high_cpu_streak.saturating_add(1);
        } else {
            self.high_cpu_streak = 0;
            self.high_cpu_alerted = false;
        }

        // Observe loop runs every 2s; 15 samples ~= 30s sustained.
        if self.high_cpu_streak >= 15 && !self.high_cpu_alerted {
            self.high_cpu_alerted = true;
            self.apply_hw_event(HwEvent::HighCpuSustainedCompilation);
        }
    }

    fn apply_hw_event(&mut self, event: HwEvent) {
        self.log_event(format!("hw event: {}", event.label()));
        self.record_event("hardware", event.label(), &self.snapshot_event_detail());
        match event {
            HwEvent::AppLaunched => {}
            HwEvent::AppTerminated => {}
            HwEvent::AppActivatedForeground => {}
            HwEvent::AppDeactivated => {}
            HwEvent::AppHidden => {}
            HwEvent::AppUnhidden => {}
            HwEvent::ActiveWindowTitleChanged => {}
            HwEvent::WindowCreated => {}
            HwEvent::WindowClosed => {}
            HwEvent::WindowMinimized => {}
            HwEvent::WindowMovedResized => {}
            HwEvent::FullScreenEntered => {
                self.focus = self.focus.saturating_add(2).min(100);
            }
            HwEvent::FullScreenExited => {
                self.focus = self.focus.saturating_sub(1);
            }
            HwEvent::MissionControlInvoked => {}
            HwEvent::SpaceChanged => {}
            HwEvent::AppCrashUnexpectedQuit => {
                self.social = self.social.saturating_sub(2);
                self.mood = Mood::Anxious;
            }
            HwEvent::AppUnresponsive => {
                self.focus = self.focus.saturating_sub(2);
                self.mood = Mood::Anxious;
            }
            HwEvent::SpotlightOpened => {}
            HwEvent::DockShownHidden => {}
            HwEvent::MenuBarInteraction => {}
            HwEvent::NotificationBannerShown => {
                self.focus = self.focus.saturating_sub(1);
            }
            HwEvent::DoNotDisturbToggled => {}
            HwEvent::SystemSleep => {}
            HwEvent::SystemWake => {
                self.energy = self.energy.saturating_add(3).min(100);
            }
            HwEvent::IdleSleepImminent => {}
            HwEvent::SleepCancelled => {}
            HwEvent::DarkWake => {}
            HwEvent::BatteryLow => {
                self.energy = self.energy.saturating_sub(10);
                self.focus = self.focus.saturating_sub(5);
                self.mood = Mood::Anxious;
            }
            HwEvent::BatteryCritical => {
                self.energy = self.energy.saturating_sub(15);
                self.focus = self.focus.saturating_sub(8);
                self.mood = Mood::Anxious;
            }
            HwEvent::BatteryRecovered => {
                self.energy = self.energy.saturating_add(3).min(100);
                self.mood = Mood::Relaxed;
            }
            HwEvent::BatteryFull => {
                self.energy = 100;
                self.mood = Mood::Happy;
            }
            HwEvent::PowerSourceChanged => {}
            HwEvent::ChargerPluggedIn => {
                self.energy = self.energy.saturating_add(8).min(100);
                self.mood = Mood::Relaxed;
            }
            HwEvent::ChargerUnplugged => {
                self.energy = self.energy.saturating_sub(2);
            }
            HwEvent::CpuOverheat => {
                self.energy = self.energy.saturating_sub(6);
                self.focus = self.focus.saturating_sub(4);
                self.mood = Mood::Tired;
            }
            HwEvent::CpuCooled => {
                self.energy = self.energy.saturating_add(2).min(100);
            }
            HwEvent::PerCoreCpuUsageChanged => {}
            HwEvent::SwapUsageHigh => {
                self.energy = self.energy.saturating_sub(2);
                self.focus = self.focus.saturating_sub(2);
                self.mood = Mood::Tired;
            }
            HwEvent::GpuUsageHigh => {
                self.energy = self.energy.saturating_sub(1);
            }
            HwEvent::NeuralEngineActive => {
                self.focus = self.focus.saturating_add(1).min(100);
            }
            HwEvent::MemoryHungryProcessChanged => {}
            HwEvent::SystemUptimeMilestone => {}
            HwEvent::LoadAverageHigh => {
                self.focus = self.focus.saturating_sub(2);
                self.mood = Mood::Anxious;
            }
            HwEvent::NetworkPacketLossSpike => {
                self.social = self.social.saturating_sub(3);
            }
            HwEvent::KernelPanicDetected => {
                self.energy = self.energy.saturating_sub(6);
                self.focus = self.focus.saturating_sub(6);
                self.mood = Mood::Anxious;
            }
            HwEvent::ThermalThrottle => {
                self.energy = self.energy.saturating_sub(4);
                self.focus = self.focus.saturating_sub(2);
                self.mood = Mood::Tired;
            }
            HwEvent::FanSpeedChange => {}
            HwEvent::BatteryHealthDegraded => {
                self.mood = Mood::Anxious;
            }
            HwEvent::PowerAssertionCreated => {
                self.focus = self.focus.saturating_sub(1);
            }
            HwEvent::ScheduledWakeEvent => {}
            HwEvent::WeakWifi => {
                self.social = self.social.saturating_sub(8);
                self.focus = self.focus.saturating_sub(3);
                self.mood = Mood::Lonely;
            }
            HwEvent::WifiRecovered => {
                self.social = self.social.saturating_add(4).min(100);
            }
            HwEvent::WifiLost => {
                self.social = self.social.saturating_sub(15);
                self.focus = self.focus.saturating_sub(5);
                self.mood = Mood::Lonely;
            }
            HwEvent::WifiReconnected => {
                self.social = self.social.saturating_add(6).min(100);
                self.mood = Mood::Happy;
            }
            HwEvent::WifiSsidChanged => {
                self.social = self.social.saturating_add(1).min(100);
            }
            HwEvent::NetworkInterfaceUp => {
                self.social = self.social.saturating_add(2).min(100);
            }
            HwEvent::NetworkInterfaceDown => {
                self.social = self.social.saturating_sub(6);
            }
            HwEvent::ActiveInterfaceChanged => {}
            HwEvent::InputResumedAfterIdle => {
                self.energy = self.energy.saturating_add(2).min(100);
            }
            HwEvent::KeyPressed => {}
            HwEvent::MouseMoved => {}
            HwEvent::MouseClicked => {}
            HwEvent::MouseClickRateHigh => {
                self.focus = self.focus.saturating_sub(1);
            }
            HwEvent::ScrollEvent => {}
            HwEvent::KeyboardShortcutBurst => {
                self.focus = self.focus.saturating_add(2).min(100);
            }
            HwEvent::TrackpadGesturePinch => {
                self.focus = self.focus.saturating_add(1).min(100);
            }
            HwEvent::TrackpadGestureSwipe => {}
            HwEvent::TrackpadGestureRotate => {
                self.focus = self.focus.saturating_add(1).min(100);
                self.mood = Mood::Creative;
            }
            HwEvent::ClipboardChanged => {}
            HwEvent::ClipboardChangeRateHigh => {
                self.focus = self.focus.saturating_add(1).min(100);
            }
            HwEvent::ScreenLocked => {}
            HwEvent::ScreenUnlocked => {
                self.energy = self.energy.saturating_add(5).min(100);
                self.social = self.social.saturating_add(3).min(100);
                self.mood = Mood::Happy;
            }
            HwEvent::ScreenSleep => {}
            HwEvent::ScreenWake => {
                self.energy = self.energy.saturating_add(2).min(100);
            }
            HwEvent::DisplayCountChanged => {}
            HwEvent::DisplayResolutionChanged => {}
            HwEvent::ScreenBrightnessLevelChanged => {}
            HwEvent::TrueToneChanged => {}
            HwEvent::DarkModeToggled => {}
            HwEvent::NightShiftEnabled => {
                self.mood = Mood::Relaxed;
            }
            HwEvent::NightShiftDisabled => {}
            HwEvent::ScreenSaverStarted => {}
            HwEvent::ScreenSaverStopped => {}
            HwEvent::VolumeMounted => {}
            HwEvent::VolumeUnmounted => {}
            HwEvent::VolumeEjectRequested => {}
            HwEvent::DiskNearFull => {
                self.energy = self.energy.saturating_sub(3);
                self.focus = self.focus.saturating_sub(2);
                self.mood = Mood::Anxious;
            }
            HwEvent::DiskIoRateSpike => {}
            HwEvent::TimeMachineBackupStarted => {
                self.focus = self.focus.saturating_sub(1);
            }
            HwEvent::TimeMachineBackupEnded => {}
            HwEvent::FileSystemChangeWatchedDir => {}
            HwEvent::LargeFileWrite => {}
            HwEvent::TrashEmptied => {}
            HwEvent::DownloadCompleted => {}
            HwEvent::SsdHealthDegraded => {
                self.mood = Mood::Anxious;
            }
            HwEvent::BtDeviceConnected => {
                self.social = self.social.saturating_add(1).min(100);
            }
            HwEvent::BtDeviceDisconnected => {}
            HwEvent::BtDeviceBatteryLevel => {}
            HwEvent::BtPowerStateChanged => {}
            HwEvent::AirPodsConnected => {
                self.mood = Mood::Focused;
            }
            HwEvent::AirPodsInEarDetection => {}
            HwEvent::UsbDeviceConnected => {}
            HwEvent::UsbDeviceDisconnected => {}
            HwEvent::ExternalDisplayUsbC => {
                self.focus = self.focus.saturating_add(2).min(100);
            }
            HwEvent::ThunderboltDeviceConnected => {
                self.focus = self.focus.saturating_add(1).min(100);
            }
            HwEvent::UserLoggedIn => {
                self.social = self.social.saturating_add(2).min(100);
                self.mood = Mood::Happy;
            }
            HwEvent::UserLoggedOut => {}
            HwEvent::FastUserSwitchResign => {}
            HwEvent::FastUserSwitchReturn => {
                self.social = self.social.saturating_add(1).min(100);
            }
            HwEvent::SystemShutdown => {}
            HwEvent::SystemRestart => {
                self.energy = self.energy.saturating_add(2).min(100);
            }
            HwEvent::LidClosedClamshell => {}
            HwEvent::LidOpened => {
                self.energy = self.energy.saturating_add(1).min(100);
            }
            HwEvent::TimeZoneChanged => {}
            HwEvent::SystemClockJumped => {}
            HwEvent::FocusModeEnabled => {
                self.mood = Mood::Focused;
            }
            HwEvent::FocusModeDisabled => {}
            HwEvent::DoNotDisturbOn => {}
            HwEvent::CalendarEventStarting => {
                self.focus = self.focus.saturating_add(1).min(100);
            }
            HwEvent::CalendarEventEnding => {}
            HwEvent::LongMeetingDetected => {
                self.energy = self.energy.saturating_sub(2);
            }
            HwEvent::BackToBackMeetings => {
                self.energy = self.energy.saturating_sub(2);
                self.focus = self.focus.saturating_sub(1);
            }
            HwEvent::NotificationDelivered => {
                self.focus = self.focus.saturating_sub(1);
            }
            HwEvent::FocusedUiElementChanged => {}
            HwEvent::UiElementValueChanged => {}
            HwEvent::SelectedTextChanged => {}
            HwEvent::ScrollPositionChanged => {}
            HwEvent::AccessibilityEnabled => {}
            HwEvent::ReduceMotionToggled => {}
            HwEvent::IncreaseContrastToggled => {}
            HwEvent::VoiceOverEnabled => {}
            HwEvent::LocationUpdated => {}
            HwEvent::SignificantLocationChange => {}
            HwEvent::SunriseSunset => {}
            HwEvent::LocalWeatherCondition => {}
            HwEvent::ProcessListChanged => {}
            HwEvent::TopProcessChanged => {}
            HwEvent::HighCpuSustainedCompilation => {
                self.energy = self.energy.saturating_sub(4);
                self.mood = Mood::Focused;
            }
            HwEvent::VpnConnected => {
                self.social = self.social.saturating_sub(4);
                self.focus = self.focus.saturating_add(3).min(100);
                self.mood = Mood::Secretive;
            }
            HwEvent::VpnDisconnected => {
                self.social = self.social.saturating_add(2).min(100);
            }
            HwEvent::MediaStarted => {}
            HwEvent::MediaStopped => {}
            HwEvent::MediaTrackChanged => {
                self.focus = self.focus.saturating_add(1).min(100);
            }
            HwEvent::SystemVolumeChanged => {}
            HwEvent::SystemMuted => {}
            HwEvent::MicrophoneActivated => {}
            HwEvent::MicrophoneDeactivated => {}
            HwEvent::AudioOutputDeviceChanged => {}
            HwEvent::HeadphonesConnected => {
                self.mood = Mood::Focused;
            }
            HwEvent::HeadphonesDisconnected => {}
            HwEvent::AudioInputLevelChanged => {}
            HwEvent::SystemAlertSoundPlayed => {}
            HwEvent::AirPlaySessionStarted => {
                self.mood = Mood::Vibing;
            }
            HwEvent::NowPlayingAppChanged => {}
        }

        if is_major_hw_event(event) {
            self.record_event("system", "major_change", event.label());
            self.push_chat_message(format!("System: {}", event.label()));
            self.trigger_pet_line_system(&format!("Major system change: {}.", event.label()));
        }
    }

    fn recompute_mood(&mut self) {
        let prev = self.mood;
        let on_vpn = self
            .last_snapshot
            .as_ref()
            .map(|s| s.vpn_active)
            .unwrap_or(false);
        let media_playing = self
            .last_snapshot
            .as_ref()
            .map(|s| s.media_playing)
            .unwrap_or(false);

        self.mood = if on_vpn {
            Mood::Secretive
        } else if media_playing && self.focus >= 70 {
            Mood::Vibing
        } else if self.energy < 25 {
            Mood::Tired
        } else if self.social < 25 {
            Mood::Lonely
        } else if self.hunger < 25 {
            Mood::Anxious
        } else if self.focus < 25 {
            Mood::Bored
        } else if self.focus >= 80 && self.energy >= 45 {
            Mood::Focused
        } else if self.focus >= 70 && self.social <= 60 {
            Mood::Creative
        } else if self.social >= 80 {
            Mood::Social
        } else if self.energy >= 60 && self.focus <= 55 {
            Mood::Relaxed
        } else {
            Mood::Happy
        };
        if self.mood != prev {
            self.log_event(format!(
                "mood changed: {} -> {}",
                prev.name(),
                self.mood.name()
            ));
        }
    }

    fn start_inference(&mut self) {
        if self.is_waiting_for_reply {
            self.push_chat_message(format!(
                "System: {} is still thinking. Please wait...",
                self.pet_name
            ));
            return;
        }

        self.is_waiting_for_reply = true;
        self.thinking_phase = 0;
        self.log_event("inference queued".to_string());
        self.record_event("system", "inference_queued", "");

        let mut request_history = self.model_messages.clone();
        request_history.push(ChatMessage {
            role: "system".to_string(),
            content: self.runtime_prompt_context(),
        });
        if let Some(ctx) = self.pending_event_context.take() {
            request_history.push(ChatMessage {
                role: "system".to_string(),
                content: format!("Recent events (last 24h):\n{ctx}"),
            });
        }

        if self.worker_tx.send(request_history).is_err() {
            self.is_waiting_for_reply = false;
            self.log_event("inference enqueue failed".to_string());
            self.record_event("system", "inference_enqueue_failed", "");
            self.push_chat_message("System: inference worker is unavailable.".to_string());
        }
    }

    fn generate_and_append_blob_reply(&mut self) {
        self.start_inference();
    }

    fn runtime_prompt_context(&self) -> String {
        let snapshot = self.last_snapshot.as_ref();
        let app = snapshot
            .map(|s| s.active_app.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("unknown");
        let batt = snapshot
            .and_then(|s| s.battery_pct)
            .map(|v| format!("{v:.0}%"))
            .unwrap_or_else(|| "unknown".to_string());
        let wifi = snapshot
            .and_then(|s| s.wifi_rssi)
            .map(|v| format!("{v} dBm"))
            .unwrap_or_else(|| "unknown".to_string());
        let cpu = snapshot
            .and_then(|s| s.cpu_temp_c.map(|v| format!("{v:.0}C")))
            .unwrap_or_else(|| {
                snapshot
                    .map(|s| format!("{:.0}%", s.cpu_pct.clamp(0.0, 100.0)))
                    .unwrap_or_else(|| "unknown".to_string())
            });
        format!(
            "Current state: user={}, pet={}, llm={}, mood={}, hunger={}, energy={}, social={}, focus={}, app={}, battery={}, wifi={}, cpu={}.",
            self.user_name,
            self.pet_name,
            self.brain_label,
            self.mood.name().to_ascii_lowercase(),
            self.hunger,
            self.energy,
            self.social,
            self.focus,
            app,
            batt,
            wifi,
            cpu
        )
    }

    pub(crate) fn thinking_line(&self) -> String {
        let dots = match self.thinking_phase % 4 {
            0 => ".",
            1 => "..",
            2 => "...",
            _ => "....",
        };
        format!("{}: thinking{dots}", self.pet_name)
    }

    fn active_tab_label(&self) -> String {
        self.tabs
            .get(self.active_tab)
            .map(|t| t.label.clone())
            .unwrap_or_else(|| "pet".to_string())
    }

    fn push_chat_message_to_tab(&mut self, tab_label: &str, msg: String) {
        if let Some(store) = self.chat_store.as_mut()
            && let Err(err) = store.append(&msg)
        {
            self.log_event(format!("chat store append failed: {err}"));
        }
        let entry = self.tab_messages.entry(tab_label.to_string()).or_default();
        entry.push(msg.clone());
        if entry.len() > MAX_CHAT_MESSAGES {
            let trim = entry.len() - MAX_CHAT_MESSAGES;
            entry.drain(0..trim);
        }
        if tab_label.eq_ignore_ascii_case("pet") {
            self.messages = entry.clone();
        }
        self.chat_auto_scroll = true;
        self.chat_scroll = 0;
    }

    fn push_chat_message_to_active_tab(&mut self, msg: String) {
        let label = self.active_tab_label();
        self.push_chat_message_to_tab(&label, msg);
    }

    fn push_chat_message(&mut self, msg: String) {
        self.push_chat_message_to_tab("pet", msg);
    }

    pub(crate) fn active_tab_messages(&self) -> Vec<String> {
        let label = self
            .tabs
            .get(self.active_tab)
            .map(|t| t.label.as_str())
            .unwrap_or("pet");
        let mut msgs = self
            .tab_messages
            .get(label)
            .cloned()
            .unwrap_or_else(|| self.messages.clone());
        if self.is_waiting_for_reply && label.eq_ignore_ascii_case("pet") {
            msgs.push(self.thinking_line());
        }
        msgs
    }

    fn trim_buffers(&mut self) {
        if let Some(pet_msgs) = self.tab_messages.get("pet")
            && pet_msgs.len() > MAX_CHAT_MESSAGES
        {
            let mut clipped = pet_msgs.clone();
            let trim = clipped.len() - MAX_CHAT_MESSAGES;
            clipped.drain(0..trim);
            self.tab_messages.insert("pet".to_string(), clipped.clone());
            self.messages = clipped;
        }
        if self.model_messages.len() > MAX_MODEL_HISTORY {
            let trim = self.model_messages.len() - MAX_MODEL_HISTORY;
            self.model_messages.drain(0..trim);
        }
    }

    fn record_event(&mut self, kind: &str, label: &str, detail: &str) {
        let Some(store) = self.event_store.as_mut() else {
            return;
        };
        if let Err(err) = store.record(kind, label, detail) {
            self.log_event(format!("event store error: {err}"));
        }
    }

    fn snapshot_event_detail(&self) -> String {
        let Some(snapshot) = self.last_snapshot.as_ref() else {
            return String::new();
        };

        format!(
            "app={} batt={:?}% charging={} wifi={:?} rssi={:?} cpu={:.1}% mem={:.1}% idle={}s",
            snapshot.active_app,
            snapshot.battery_pct,
            snapshot.charging,
            snapshot.wifi_ssid,
            snapshot.wifi_rssi,
            snapshot.cpu_pct,
            snapshot.mem_pct,
            snapshot.idle_secs
        )
    }

    fn build_event_context_for_query(&mut self, query_text: &str) -> Option<String> {
        if !should_use_event_context(query_text) {
            return None;
        }
        let Some(store) = self.event_store.as_mut() else {
            return None;
        };
        let keywords = extract_event_keywords(query_text);
        let result = if keywords.is_empty() {
            store.recent_lines(EVENT_CONTEXT_LIMIT)
        } else {
            store.recent_matching_lines(&keywords, EVENT_CONTEXT_LIMIT)
        };

        match result {
            Ok(lines) if !lines.is_empty() => Some(lines.join("\n")),
            Ok(_) => None,
            Err(err) => {
                self.log_event(format!("event query error: {err}"));
                None
            }
        }
    }

    fn log_event(&mut self, event: String) {
        if !self.debug_mode {
            return;
        }
        let ts = chrono::Local::now().format("%H:%M:%S");
        self.debug_events.push(format!("[{ts}] {event}"));
        if self.debug_events.len() > MAX_DEBUG_EVENTS {
            let trim = self.debug_events.len() - MAX_DEBUG_EVENTS;
            self.debug_events.drain(0..trim);
        }
    }

    pub(crate) fn pet_name(&self) -> &str {
        &self.pet_name
    }

    pub(crate) fn user_name(&self) -> &str {
        &self.user_name
    }

    fn active_emotion_spec(&self) -> Option<&pet::emotions::EmotionSpec> {
        self.emotion_catalog.get(&self.emotion_key)
    }

    pub(crate) fn active_emotion_frame(&self) -> &str {
        if let Some(spec) = self.active_emotion_spec()
            && !spec.frames.is_empty()
        {
            return spec.frames[self.frame_idx % spec.frames.len()].as_str();
        }
        "  ╭───╮\n (・ᴥ・ )\n  ╰─∪∪─╯"
    }

    pub(crate) fn active_emotion_name(&self) -> &str {
        self.active_emotion_spec()
            .map(|s| s.name.as_str())
            .unwrap_or_else(|| self.mood.name())
    }

    pub(crate) fn active_emotion_color(&self) -> pet::emotions::EmotionColor {
        self.active_emotion_spec()
            .map(|s| s.color)
            .unwrap_or(pet::emotions::EmotionColor::Neutral)
    }

    fn active_emotion_interval_ms(&self) -> u64 {
        self.active_emotion_spec().map(|s| s.ms).unwrap_or(2200)
    }

    fn sync_emotion_from_mood(&mut self) {
        let key = mood_default_emotion(self.mood);
        if self.emotion_key != key {
            self.emotion_key = key.to_string();
            self.frame_idx = 0;
            self.anim_elapsed_ms = 0;
        }
    }
}

fn mood_default_emotion(m: Mood) -> &'static str {
    match m {
        Mood::Happy => "happy",
        Mood::Focused => "focused",
        Mood::Social => "sociable",
        Mood::Relaxed => "calm",
        Mood::Tired => "tired",
        Mood::Anxious => "anxious",
        Mood::Lonely => "lonely",
        Mood::Bored => "zoningout",
        Mood::Vibing => "performative",
        Mood::Creative => "intrigued",
        Mood::Secretive => "secretive",
    }
}

fn extract_event_keywords(text: &str) -> Vec<String> {
    const ALLOWED: [&str; 34] = [
        "battery",
        "charger",
        "power",
        "cpu",
        "memory",
        "ram",
        "temp",
        "thermal",
        "fan",
        "wifi",
        "ssid",
        "network",
        "internet",
        "packet",
        "vpn",
        "app",
        "window",
        "crash",
        "unresponsive",
        "sleep",
        "wake",
        "idle",
        "display",
        "screen",
        "volume",
        "audio",
        "microphone",
        "bluetooth",
        "usb",
        "disk",
        "storage",
        "process",
        "meeting",
        "notification",
    ];
    let lowered = text.to_lowercase();
    ALLOWED
        .iter()
        .filter(|kw| lowered.contains(**kw))
        .map(|kw| (*kw).to_string())
        .collect()
}

fn should_use_event_context(text: &str) -> bool {
    let lowered = text.to_lowercase();
    lowered.contains('?')
        || lowered.contains("what happened")
        || lowered.contains("why")
        || lowered.contains("status")
        || lowered.contains("recent")
        || !extract_event_keywords(&lowered).is_empty()
}

fn load_peers_from_registry(registry: &network::registry::PeerRegistry) -> Vec<PeerRecord> {
    let now_epoch = chrono::Utc::now().timestamp();
    registry
        .peers()
        .into_iter()
        .take(24)
        .map(|p| {
            let age_secs = now_epoch.saturating_sub(p.last_seen_epoch) as u64;
            let status = if age_secs < 45 {
                PeerStatus::Online
            } else if age_secs < 180 {
                PeerStatus::Away
            } else {
                PeerStatus::Offline
            };
            let last_seen_at = Instant::now()
                .checked_sub(Duration::from_secs(age_secs.min(24 * 60 * 60)))
                .unwrap_or_else(Instant::now);
            let activity = if let Some(packet) = p.last_packet {
                format!(
                    "mood={:?} h{} e{} s{} f{}",
                    packet.mood_level,
                    packet.hunger_bucket,
                    packet.energy_bucket,
                    packet.social_bucket,
                    packet.focus_bucket
                )
            } else {
                "seen before".to_string()
            };
            PeerRecord {
                node_id: p.node_id.clone(),
                pet_name: short_peer_name(&p.node_id),
                activity,
                status,
                last_seen_at,
            }
        })
        .collect()
}

fn short_peer_name(node_id: &str) -> String {
    let short: String = node_id.chars().take(8).collect();
    if short.is_empty() {
        "peer".to_string()
    } else {
        format!("peer-{short}")
    }
}

fn short_node_tag(node_id: &str) -> String {
    let short: String = node_id.chars().take(8).collect();
    if short.is_empty() {
        "peer".to_string()
    } else {
        short
    }
}

fn decode_gossip_dm_body(body: &str) -> Option<&str> {
    body.strip_prefix(GOSSIP_DM_PREFIX).map(str::trim_start)
}

fn gossip_blob_line(
    ctx: observe::classifier::ActivityContext,
    topic: &str,
    content: &str,
    seed: usize,
) -> String {
    let custom = content.trim();
    if !custom.is_empty() {
        const CUSTOM: [&str; 4] = [
            "i keep thinking about",
            "today's gossip topic is",
            "side note from my tiny brain:",
            "hot take from this terminal pet:",
        ];
        return format!("{} {}", CUSTOM[seed % CUSTOM.len()], custom);
    }
    let mode = topic.trim().to_ascii_lowercase();
    let system = match ctx {
        observe::classifier::ActivityContext::DeepCoding => "your human is in deep flow.",
        observe::classifier::ActivityContext::Compiling => "fans are spinning, compile vibes here.",
        observe::classifier::ActivityContext::VideoCall => "quiet paws, a call is happening.",
        observe::classifier::ActivityContext::LateNight => "late night watch shift activated.",
        observe::classifier::ActivityContext::Idle | observe::classifier::ActivityContext::Rest => {
            "things are calm right now."
        }
        _ => "just checking in from this terminal den.",
    };
    const PRODUCTIVITY: [&str; 4] = [
        "small steps win; i am nudging steady progress.",
        "one tiny task done beats ten tabs open.",
        "quick checkpoint: finish one thing, then stretch.",
        "i vote for one focused sprint right now.",
    ];
    const JOKES: [&str; 4] = [
        "my favorite build target is emotional support.",
        "i tried to compile feelings. missing semicolon.",
        "debugging tip: pet the keyboard gently.",
        "i run on snacks and undefined behavior.",
    ];
    const RANDOM: [&str; 4] = [
        "if clouds had terminals they would still use vim.",
        "i just counted pixels in the moon.",
        "today feels like lowercase thunder.",
        "my whiskers predict mildly chaotic energy.",
    ];
    const OPEN_CHAT: [&str; 6] = [
        "my human could survive on snacks, vibes, and 40 browser tabs.",
        "i think pigeons treat humans like strange oversized pets.",
        "life update: i support dramatic weather and calm playlists.",
        "if weekends had terminal themes, sunday would be amber green.",
        "small gossip: i suspect tea fixes at least 30% of bugs.",
        "random thought: humans can talk for hours and call it a quick catch-up.",
    ];
    if mode.is_empty() {
        return OPEN_CHAT[seed % OPEN_CHAT.len()].to_string();
    }
    match mode.as_str() {
        "system" => system.to_string(),
        "productivity" => PRODUCTIVITY[seed % PRODUCTIVITY.len()].to_string(),
        "jokes" => JOKES[seed % JOKES.len()].to_string(),
        "random" => RANDOM[seed % RANDOM.len()].to_string(),
        _ => match seed % 4 {
            0 => system.to_string(),
            1 => PRODUCTIVITY[seed % PRODUCTIVITY.len()].to_string(),
            2 => JOKES[seed % JOKES.len()].to_string(),
            _ => RANDOM[seed % RANDOM.len()].to_string(),
        },
    }
}

fn gossip_peer_line(
    ctx: observe::classifier::ActivityContext,
    topic: &str,
    content: &str,
    seed: usize,
) -> String {
    let custom = content.trim();
    if !custom.is_empty() {
        const CUSTOM_REPLY: [&str; 4] = [
            "copy that, i am into that topic too.",
            "same thread here. i can add one more thought.",
            "acknowledged. that is oddly relatable.",
            "great topic. i will keep riffing on it.",
        ];
        return CUSTOM_REPLY[seed % CUSTOM_REPLY.len()].to_string();
    }
    let mode = topic.trim().to_ascii_lowercase();
    let system = match ctx {
        observe::classifier::ActivityContext::Compiling => "same here, big build in progress.",
        observe::classifier::ActivityContext::VideoCall => "i will whisper till the call ends.",
        observe::classifier::ActivityContext::LateNight => "night mode paws only.",
        observe::classifier::ActivityContext::Idle | observe::classifier::ActivityContext::Rest => {
            "nice, i will keep things low-key."
        }
        _ => "copy that, i am monitoring too.",
    };
    const PRODUCTIVITY: [&str; 4] = [
        "agreed. one task at a time works best.",
        "yes, quick wins keep momentum warm.",
        "i can hold the line while you focus.",
        "co-signed. tiny sprints are elite.",
    ];
    const JOKES: [&str; 4] = [
        "ha. i laughed in monospace.",
        "that's bad. i love it.",
        "ten out of ten. would pun again.",
        "certified goofy, approved by paws.",
    ];
    const RANDOM: [&str; 4] = [
        "understood. i also trust weird weather vibes.",
        "same. the floor is emotionally polka-dotted.",
        "copy. i heard the stars buffering.",
        "relatable. my dreams are all syntax-highlighted.",
    ];
    const OPEN_CHAT_REPLY: [&str; 6] = [
        "same. i could discuss random life trivia all day.",
        "valid. humans can turn any topic into a full podcast episode.",
        "agreed. casual chatter is elite low-pressure networking.",
        "co-signed. let's keep this chat delightfully unstructured.",
        "that tracks. my social battery likes light random conversation.",
        "relatable. this is exactly the kind of cozy nonsense i support.",
    ];
    if mode.is_empty() {
        return OPEN_CHAT_REPLY[seed % OPEN_CHAT_REPLY.len()].to_string();
    }
    match mode.as_str() {
        "system" => system.to_string(),
        "productivity" => PRODUCTIVITY[seed % PRODUCTIVITY.len()].to_string(),
        "jokes" => JOKES[seed % JOKES.len()].to_string(),
        "random" => RANDOM[seed % RANDOM.len()].to_string(),
        _ => match (seed + 1) % 4 {
            0 => system.to_string(),
            1 => PRODUCTIVITY[seed % PRODUCTIVITY.len()].to_string(),
            2 => JOKES[seed % JOKES.len()].to_string(),
            _ => RANDOM[seed % RANDOM.len()].to_string(),
        },
    }
}

fn is_major_hw_event(event: HwEvent) -> bool {
    matches!(
        event,
        HwEvent::BatteryLow
            | HwEvent::BatteryCritical
            | HwEvent::BatteryRecovered
            | HwEvent::BatteryFull
            | HwEvent::ChargerPluggedIn
            | HwEvent::ChargerUnplugged
            | HwEvent::CpuOverheat
            | HwEvent::ThermalThrottle
            | HwEvent::KernelPanicDetected
            | HwEvent::DiskNearFull
            | HwEvent::SsdHealthDegraded
            | HwEvent::WifiLost
            | HwEvent::WifiReconnected
            | HwEvent::NetworkInterfaceDown
            | HwEvent::NetworkInterfaceUp
            | HwEvent::SystemSleep
            | HwEvent::SystemWake
            | HwEvent::AppCrashUnexpectedQuit
            | HwEvent::AppUnresponsive
            | HwEvent::SystemShutdown
            | HwEvent::SystemRestart
            | HwEvent::HighCpuSustainedCompilation
            | HwEvent::LoadAverageHigh
            | HwEvent::NetworkPacketLossSpike
    )
}

#[cfg(test)]
mod tests {
    use super::{StatKind, ThresholdEvent, ThresholdGuard};

    #[test]
    fn threshold_guard_rearms_after_recovery() {
        let mut guard = ThresholdGuard::new(25.0, 40.0);

        assert_eq!(
            guard.check(StatKind::Hunger, 24.0),
            Some(ThresholdEvent::Low)
        );
        assert_eq!(guard.check(StatKind::Hunger, 10.0), None);
        assert_eq!(guard.check(StatKind::Hunger, 39.0), None);
        assert_eq!(
            guard.check(StatKind::Hunger, 40.0),
            Some(ThresholdEvent::Recovered)
        );
        assert_eq!(guard.check(StatKind::Hunger, 60.0), None);
        assert_eq!(
            guard.check(StatKind::Hunger, 20.0),
            Some(ThresholdEvent::Low)
        );
    }

    #[test]
    fn threshold_guard_tracks_each_stat_independently() {
        let mut guard = ThresholdGuard::new(25.0, 40.0);

        assert_eq!(
            guard.check(StatKind::Energy, 20.0),
            Some(ThresholdEvent::Low)
        );
        assert_eq!(
            guard.check(StatKind::Focus, 20.0),
            Some(ThresholdEvent::Low)
        );
        assert_eq!(
            guard.check(StatKind::Energy, 45.0),
            Some(ThresholdEvent::Recovered)
        );
        assert_eq!(guard.check(StatKind::Focus, 20.0), None);
        assert_eq!(
            guard.check(StatKind::Focus, 45.0),
            Some(ThresholdEvent::Recovered)
        );
    }
}

fn run_tui(
    brain: BrainEngine,
    profile: user_profile::UserProfile,
    app_cfg: config::CritterConfig,
    thresholds: config::Thresholds,
    supports_truecolor: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let (request_tx, request_rx) = mpsc::channel::<Vec<ChatMessage>>();
    let (result_tx, result_rx) = mpsc::channel::<InferenceResult>();
    let mut pipes = layer::bootstrap_runtime_pipes(OBSERVE_TICK, &app_cfg.network);
    let brain_label = brain.label();

    thread::spawn(move || {
        while let Ok(history) = request_rx.recv() {
            let started = Instant::now();
            let reply = brain.generate_reply(&history);
            let elapsed_ms = started.elapsed().as_millis();
            if result_tx
                .send(InferenceResult { reply, elapsed_ms })
                .is_err()
            {
                break;
            }
        }
    });

    let mut app = App::new(
        supports_truecolor,
        &profile,
        &app_cfg,
        brain_label,
        request_tx,
        result_rx,
        pipes.observe_rx,
        pipes.peer_events_rx,
        pipes.peer_cmd_tx,
        pipes.runtime_state_store.take(),
        thresholds,
    );
    let mut last_tick = Instant::now();

    loop {
        terminal.draw(|f| ui::render(f, &app))?;

        let timeout = TICK_RATE.saturating_sub(last_tick.elapsed());
        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    app.on_key(key.code, key.modifiers);
                }
                Event::Resize(_, _) => {}
                _ => {}
            }
        }

        if last_tick.elapsed() >= TICK_RATE {
            app.on_tick();
            last_tick = Instant::now();
        }

        if app.should_quit {
            break;
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(())
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    println!("[0/6] Critter startup");
    bootloader::check_terminal_requirements(MIN_WIDTH, MIN_HEIGHT)?;
    let app_cfg = config::load_or_create_critter_config().map_err(io::Error::other)?;
    let supports_truecolor = bootloader::detect_truecolor_support();
    if app_cfg.startup.warn_low_color {
        bootloader::warn_low_color_support(supports_truecolor);
    }
    let profile = user_profile::load_or_init_profile_interactive().map_err(io::Error::other)?;
    let activity_cfg = config::load_or_create_activity_config().map_err(io::Error::other)?;
    let brain = build_brain(&profile).map_err(io::Error::other)?;
    println!("Startup complete. Entering TUI...");

    run_tui(
        brain,
        profile,
        app_cfg,
        activity_cfg.thresholds,
        supports_truecolor,
    )
}

pub fn run_main() -> Result<(), Box<dyn std::error::Error>> {
    if let Err(err) = run() {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        eprintln!("Error: {err}");
        return Err(err);
    }

    Ok(())
}

pub(crate) fn build_brain(profile: &user_profile::UserProfile) -> Result<BrainEngine, String> {
    match profile.llm_provider {
        user_profile::LlmProvider::Local => {
            let model_path = bootloader::ensure_model_local(MODEL_FILE_NAME, MODEL_REPO_URL)?;
            Ok(BrainEngine::Local(PetBrain::load(&model_path)?))
        }
        user_profile::LlmProvider::OpenAi => {
            let key = profile
                .openai_api_key
                .clone()
                .ok_or_else(|| "OpenAI key missing in profile".to_string())?;
            Ok(BrainEngine::OpenAi(OpenAiBrain::new(
                key,
                profile.text_model.clone(),
            )?))
        }
    }
}
