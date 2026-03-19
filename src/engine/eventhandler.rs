use crate::{core::runtime::HwEvent, observe};
use std::time::Duration;

pub(crate) fn detect_hw_events(
    prev: Option<&observe::snapshot::OsSnapshot>,
    current: &observe::snapshot::OsSnapshot,
    observe_tick: Duration,
) -> Vec<HwEvent> {
    let mut events = Vec::new();

    let Some(prev) = prev else {
        // No transition events on first sample; we log initial state separately.
        return events;
    };

    if current.process_count > prev.process_count {
        events.push(HwEvent::AppLaunched);
    } else if current.process_count < prev.process_count {
        events.push(HwEvent::AppTerminated);
        if current.active_app != prev.active_app && prev.app_unresponsive {
            events.push(HwEvent::AppCrashUnexpectedQuit);
        }
    }

    if current.active_app != prev.active_app {
        events.push(HwEvent::AppDeactivated);
        events.push(HwEvent::AppActivatedForeground);
        events.push(HwEvent::AppHidden);
        events.push(HwEvent::AppUnhidden);
    }

    if current.active_title != prev.active_title
        && !current.active_title.is_empty()
        && current.active_app == prev.active_app
    {
        events.push(HwEvent::ActiveWindowTitleChanged);
    }
    if current.active_window_count > prev.active_window_count
        && current.active_app == prev.active_app
    {
        events.push(HwEvent::WindowCreated);
    }
    if current.active_window_count < prev.active_window_count
        && current.active_app == prev.active_app
    {
        events.push(HwEvent::WindowClosed);
        events.push(HwEvent::WindowMinimized);
    }
    if current.active_window_geom_sig != prev.active_window_geom_sig
        && current.active_window_count == prev.active_window_count
        && current.active_app == prev.active_app
    {
        events.push(HwEvent::WindowMovedResized);
    }

    let prev_fullscreen = looks_fullscreen(prev);
    let current_fullscreen = looks_fullscreen(current);
    if current_fullscreen && !prev_fullscreen {
        events.push(HwEvent::FullScreenEntered);
    }
    if !current_fullscreen && prev_fullscreen {
        events.push(HwEvent::FullScreenExited);
    }
    if current.mission_control_active && !prev.mission_control_active {
        events.push(HwEvent::MissionControlInvoked);
    }
    if current.active_space_id != prev.active_space_id
        && current.active_space_id.is_some()
        && prev.active_space_id.is_some()
    {
        events.push(HwEvent::SpaceChanged);
        events.push(HwEvent::TrackpadGestureSwipe);
    }
    if current.app_unresponsive && !prev.app_unresponsive {
        events.push(HwEvent::AppUnresponsive);
    }
    if current.spotlight_opened && !prev.spotlight_opened {
        events.push(HwEvent::SpotlightOpened);
    }
    if current.dock_autohide != prev.dock_autohide {
        events.push(HwEvent::DockShownHidden);
    }
    if current.dnd_enabled != prev.dnd_enabled {
        events.push(HwEvent::DoNotDisturbToggled);
    }
    if current.dnd_enabled && !prev.dnd_enabled {
        events.push(HwEvent::FocusModeEnabled);
        events.push(HwEvent::DoNotDisturbOn);
    }
    if !current.dnd_enabled && prev.dnd_enabled {
        events.push(HwEvent::FocusModeDisabled);
    }
    if current.notification_center_visible && !prev.notification_center_visible {
        events.push(HwEvent::NotificationBannerShown);
        events.push(HwEvent::NotificationDelivered);
    }
    if current.active_title.is_empty()
        && !prev.active_title.is_empty()
        && current.active_app == prev.active_app
    {
        events.push(HwEvent::MenuBarInteraction);
    }

    let prev_imminent = sleep_imminent(prev);
    let cur_imminent = sleep_imminent(current);

    let wall_gap_secs = current.ts.saturating_sub(prev.ts);
    let idle_gap_secs = current.idle_secs.saturating_sub(prev.idle_secs);
    let min_gap_for_suspend = observe_tick.as_secs().saturating_mul(8).max(20);
    let likely_suspend_resume =
        wall_gap_secs >= min_gap_for_suspend && idle_gap_secs.saturating_add(5) >= wall_gap_secs;

    if likely_suspend_resume {
        // We only infer a sleep-start event when prior state already suggested imminent sleep.
        // Otherwise long poll delays can create false positives.
        if prev_imminent || prev.screen_locked || prev.idle_secs >= 300 {
            events.push(HwEvent::SystemSleep);
        }
        if current.screen_locked {
            events.push(HwEvent::DarkWake);
        } else {
            events.push(HwEvent::SystemWake);
        }
    }
    if cur_imminent && !prev_imminent {
        events.push(HwEvent::IdleSleepImminent);
    }
    if prev_imminent && !cur_imminent && current.idle_secs < 30 {
        events.push(HwEvent::SleepCancelled);
    }

    if current.battery_pct.is_some_and(|v| v < 20.0)
        && prev.battery_pct.is_none_or(|prev_v| prev_v >= 20.0)
    {
        events.push(HwEvent::BatteryLow);
    }
    if current.battery_pct.is_some_and(|v| v < 10.0)
        && prev.battery_pct.is_none_or(|prev_v| prev_v >= 10.0)
    {
        events.push(HwEvent::BatteryCritical);
    }
    if current.battery_pct.is_some_and(|v| v >= 25.0)
        && prev.battery_pct.is_some_and(|prev_v| prev_v < 20.0)
    {
        events.push(HwEvent::BatteryRecovered);
    }
    if current.battery_pct.is_some_and(|v| v >= 99.5)
        && prev.battery_pct.is_none_or(|prev_v| prev_v < 99.5)
    {
        events.push(HwEvent::BatteryFull);
    }
    if current.charging != prev.charging {
        events.push(HwEvent::PowerSourceChanged);
    }
    if current.charging && !prev.charging {
        events.push(HwEvent::ChargerPluggedIn);
    }
    if !current.charging && prev.charging {
        events.push(HwEvent::ChargerUnplugged);
    }
    if current.cpu_temp_c.is_some_and(|v| v > 85.0)
        && prev.cpu_temp_c.is_none_or(|prev_v| prev_v <= 85.0)
    {
        events.push(HwEvent::CpuOverheat);
    }
    if current.cpu_temp_c.is_some_and(|v| v < 80.0)
        && prev.cpu_temp_c.is_some_and(|prev_v| prev_v > 85.0)
    {
        events.push(HwEvent::CpuCooled);
    }
    if current.per_core_cpu_sig != prev.per_core_cpu_sig
        && current.per_core_cpu_sig.is_some()
        && prev.per_core_cpu_sig.is_some()
    {
        events.push(HwEvent::PerCoreCpuUsageChanged);
    }
    if current.swap_used_pct.is_some_and(|v| v >= 40.0)
        && prev.swap_used_pct.is_none_or(|v| v < 40.0)
    {
        events.push(HwEvent::SwapUsageHigh);
    }
    if current.gpu_pct.is_some_and(|v| v >= 75.0) && prev.gpu_pct.is_none_or(|v| v < 75.0) {
        events.push(HwEvent::GpuUsageHigh);
    }
    if current.neural_engine_pct.is_some_and(|v| v >= 1.0)
        && prev.neural_engine_pct.is_none_or(|v| v < 1.0)
    {
        events.push(HwEvent::NeuralEngineActive);
    }
    if current.memory_hungry_process_sig != prev.memory_hungry_process_sig
        && current.memory_hungry_process_sig.is_some()
        && prev.memory_hungry_process_sig.is_some()
    {
        events.push(HwEvent::MemoryHungryProcessChanged);
    }
    if let (Some(cur), Some(old)) = (current.system_uptime_secs, prev.system_uptime_secs)
        && cur / 3600 > old / 3600
    {
        events.push(HwEvent::SystemUptimeMilestone);
    }
    if current.load_avg_1m.is_some_and(|v| v >= 4.0) && prev.load_avg_1m.is_none_or(|v| v < 4.0) {
        events.push(HwEvent::LoadAverageHigh);
    }
    if current.thermal_throttled && !prev.thermal_throttled {
        events.push(HwEvent::ThermalThrottle);
    }
    if current.fan_rpm != prev.fan_rpm
        && current.fan_rpm.is_some()
        && prev.fan_rpm.is_some()
        && current
            .fan_rpm
            .unwrap_or_default()
            .abs_diff(prev.fan_rpm.unwrap_or_default())
            >= 300
    {
        events.push(HwEvent::FanSpeedChange);
    }
    if current.battery_health_pct.is_some_and(|v| v < 85.0)
        && prev.battery_health_pct.is_none_or(|v| v >= 85.0)
    {
        events.push(HwEvent::BatteryHealthDegraded);
    }
    if current.power_assertions_active && !prev.power_assertions_active {
        events.push(HwEvent::PowerAssertionCreated);
    }
    if current.scheduled_wake_count > prev.scheduled_wake_count {
        events.push(HwEvent::ScheduledWakeEvent);
    }
    if current.wifi_rssi.is_some_and(|rssi| rssi < -75)
        && prev.wifi_rssi.is_none_or(|prev_rssi| prev_rssi >= -75)
    {
        events.push(HwEvent::WeakWifi);
    }
    if current.wifi_rssi.is_some_and(|rssi| rssi >= -70)
        && prev.wifi_rssi.is_some_and(|prev_rssi| prev_rssi < -75)
    {
        events.push(HwEvent::WifiRecovered);
    }
    if current.wifi_rssi.is_none() && prev.wifi_rssi.is_some() {
        events.push(HwEvent::WifiLost);
    }
    if current.wifi_rssi.is_some() && prev.wifi_rssi.is_none() {
        events.push(HwEvent::WifiReconnected);
    }
    if current.wifi_ssid != prev.wifi_ssid
        && current.wifi_ssid.is_some()
        && prev.wifi_ssid.is_some()
    {
        events.push(HwEvent::WifiSsidChanged);
    }
    if current.network_up && !prev.network_up {
        events.push(HwEvent::NetworkInterfaceUp);
    }
    if !current.network_up && prev.network_up {
        events.push(HwEvent::NetworkInterfaceDown);
    }
    if current.active_interface != prev.active_interface
        && current.active_interface.is_some()
        && prev.active_interface.is_some()
    {
        events.push(HwEvent::ActiveInterfaceChanged);
    }
    if prev.idle_secs >= 120 && current.idle_secs < 5 {
        events.push(HwEvent::InputResumedAfterIdle);
    }
    if current.key_event_age_s < 0.15 && prev.key_event_age_s >= 0.60 {
        events.push(HwEvent::KeyPressed);
    }
    if current.mouse_move_age_s < 0.20 && prev.mouse_move_age_s >= 1.00 {
        events.push(HwEvent::MouseMoved);
    }
    if current.mouse_click_age_s < 0.20 && prev.mouse_click_age_s >= 0.80 {
        events.push(HwEvent::MouseClicked);
    }
    if current.mouse_click_rate >= 6 && prev.mouse_click_rate < 6 {
        events.push(HwEvent::MouseClickRateHigh);
    }
    if current.scroll_event_age_s < 0.20 && prev.scroll_event_age_s >= 1.00 {
        events.push(HwEvent::ScrollEvent);
    }
    if current.shortcut_burst && !prev.shortcut_burst {
        events.push(HwEvent::KeyboardShortcutBurst);
    }
    if current.active_window_geom_sig != prev.active_window_geom_sig
        && current.active_window_count == prev.active_window_count
        && current.active_app == prev.active_app
        && is_likely_pinch(
            prev.active_window_geom_sig.as_deref(),
            current.active_window_geom_sig.as_deref(),
        )
    {
        events.push(HwEvent::TrackpadGesturePinch);
    }
    if (current.active_app.eq_ignore_ascii_case("Preview")
        || current.active_app.to_ascii_lowercase().contains("photos")
        || current.active_app.to_ascii_lowercase().contains("figma"))
        && current.scroll_event_age_s < 0.25
        && prev.scroll_event_age_s >= 1.0
    {
        events.push(HwEvent::TrackpadGestureRotate);
    }
    if current.clipboard_changed {
        events.push(HwEvent::ClipboardChanged);
    }
    if current.clipboard_change_rate >= 4 && prev.clipboard_change_rate < 4 {
        events.push(HwEvent::ClipboardChangeRateHigh);
    }
    if current.screen_locked && !prev.screen_locked {
        events.push(HwEvent::ScreenLocked);
        events.push(HwEvent::ScreenSleep);
    }
    if !current.screen_locked && prev.screen_locked {
        events.push(HwEvent::ScreenUnlocked);
        events.push(HwEvent::ScreenWake);
    }
    if current.display_count != prev.display_count
        && current.display_count > 0
        && prev.display_count > 0
    {
        events.push(HwEvent::DisplayCountChanged);
    }
    if current.display_resolution_sig != prev.display_resolution_sig
        && current.display_resolution_sig.is_some()
        && prev.display_resolution_sig.is_some()
    {
        events.push(HwEvent::DisplayResolutionChanged);
    }
    if let (Some(cur), Some(old)) = (current.screen_brightness_pct, prev.screen_brightness_pct)
        && (cur - old).abs() >= 3.0
    {
        events.push(HwEvent::ScreenBrightnessLevelChanged);
    }
    if current.true_tone_sig != prev.true_tone_sig
        && current.true_tone_sig.is_some()
        && prev.true_tone_sig.is_some()
    {
        events.push(HwEvent::TrueToneChanged);
    }
    if current.dark_mode != prev.dark_mode {
        events.push(HwEvent::DarkModeToggled);
    }
    if current.night_shift_enabled && !prev.night_shift_enabled {
        events.push(HwEvent::NightShiftEnabled);
    }
    if !current.night_shift_enabled && prev.night_shift_enabled {
        events.push(HwEvent::NightShiftDisabled);
    }
    if current.screensaver_active && !prev.screensaver_active {
        events.push(HwEvent::ScreenSaverStarted);
    }
    if !current.screensaver_active && prev.screensaver_active {
        events.push(HwEvent::ScreenSaverStopped);
    }
    if current.external_volume_count > prev.external_volume_count {
        events.push(HwEvent::VolumeMounted);
    }
    if current.external_volume_count < prev.external_volume_count {
        events.push(HwEvent::VolumeUnmounted);
        events.push(HwEvent::VolumeEjectRequested);
    }
    if current.disk_used_pct >= 90.0 && prev.disk_used_pct < 90.0 {
        events.push(HwEvent::DiskNearFull);
    }
    if current.disk_io_mbps.is_some_and(|v| v >= 80.0) && prev.disk_io_mbps.is_none_or(|v| v < 80.0)
    {
        events.push(HwEvent::DiskIoRateSpike);
    }
    if current.time_machine_running && !prev.time_machine_running {
        events.push(HwEvent::TimeMachineBackupStarted);
    }
    if !current.time_machine_running && prev.time_machine_running {
        events.push(HwEvent::TimeMachineBackupEnded);
    }
    if current.watched_dir_sig != prev.watched_dir_sig
        && current.watched_dir_sig.is_some()
        && prev.watched_dir_sig.is_some()
    {
        events.push(HwEvent::FileSystemChangeWatchedDir);
    }
    if current.large_write_sig != prev.large_write_sig
        && current.large_write_sig.is_some()
        && prev.large_write_sig.is_some()
    {
        events.push(HwEvent::LargeFileWrite);
    }
    if current.trash_item_count == 0 && prev.trash_item_count > 0 {
        events.push(HwEvent::TrashEmptied);
    }
    if current.downloads_sig != prev.downloads_sig
        && current.downloads_sig.is_some()
        && prev.downloads_sig.is_some()
    {
        events.push(HwEvent::DownloadCompleted);
    }
    if !current.ssd_healthy && prev.ssd_healthy {
        events.push(HwEvent::SsdHealthDegraded);
    }
    if current.bluetooth_connected_count > prev.bluetooth_connected_count {
        events.push(HwEvent::BtDeviceConnected);
    }
    if current.bluetooth_connected_count < prev.bluetooth_connected_count {
        events.push(HwEvent::BtDeviceDisconnected);
    }
    if current.bluetooth_battery_sig != prev.bluetooth_battery_sig
        && current.bluetooth_battery_sig.is_some()
        && prev.bluetooth_battery_sig.is_some()
    {
        events.push(HwEvent::BtDeviceBatteryLevel);
    }
    if current.bluetooth_power_on != prev.bluetooth_power_on {
        events.push(HwEvent::BtPowerStateChanged);
    }
    if current.airpods_connected && !prev.airpods_connected {
        events.push(HwEvent::AirPodsConnected);
    }
    if current.airpods_in_ear != prev.airpods_in_ear {
        events.push(HwEvent::AirPodsInEarDetection);
    }
    if current.usb_device_count > prev.usb_device_count {
        events.push(HwEvent::UsbDeviceConnected);
    }
    if current.usb_device_count < prev.usb_device_count {
        events.push(HwEvent::UsbDeviceDisconnected);
    }
    if current.external_display_usbc && !prev.external_display_usbc {
        events.push(HwEvent::ExternalDisplayUsbC);
    }
    if current.thunderbolt_connected_count > prev.thunderbolt_connected_count {
        events.push(HwEvent::ThunderboltDeviceConnected);
    }
    let prev_user = normalized_console_user(prev.console_user.as_deref());
    let cur_user = normalized_console_user(current.console_user.as_deref());
    if cur_user != prev_user {
        match (prev_user, cur_user) {
            (None, Some(_)) => events.push(HwEvent::UserLoggedIn),
            (Some(_), None) => events.push(HwEvent::UserLoggedOut),
            (Some(_), Some(_)) => {
                events.push(HwEvent::FastUserSwitchResign);
                events.push(HwEvent::FastUserSwitchReturn);
            }
            (None, None) => {}
        }
    }
    if current.shutdown_sig != prev.shutdown_sig
        && current.shutdown_sig.is_some()
        && prev.shutdown_sig.is_some()
    {
        events.push(HwEvent::SystemShutdown);
    }
    if current.boot_session_uuid != prev.boot_session_uuid
        && current.boot_session_uuid.is_some()
        && prev.boot_session_uuid.is_some()
    {
        events.push(HwEvent::SystemRestart);
    } else if current.restart_sig != prev.restart_sig
        && current.restart_sig.is_some()
        && prev.restart_sig.is_some()
    {
        events.push(HwEvent::SystemRestart);
    }
    if current.clamshell_closed && !prev.clamshell_closed {
        events.push(HwEvent::LidClosedClamshell);
    }
    if !current.clamshell_closed && prev.clamshell_closed {
        events.push(HwEvent::LidOpened);
    }
    if current.timezone_sig != prev.timezone_sig
        && current.timezone_sig.is_some()
        && prev.timezone_sig.is_some()
    {
        events.push(HwEvent::TimeZoneChanged);
    }
    if current.calendar_active_sig != prev.calendar_active_sig {
        if prev.calendar_active_count == 0 && current.calendar_active_count > 0 {
            events.push(HwEvent::CalendarEventStarting);
        } else if prev.calendar_active_count > 0 && current.calendar_active_count == 0 {
            events.push(HwEvent::CalendarEventEnding);
        } else if current.calendar_active_count > prev.calendar_active_count {
            events.push(HwEvent::CalendarEventStarting);
        } else if current.calendar_active_count < prev.calendar_active_count {
            events.push(HwEvent::CalendarEventEnding);
        }
    }
    if current.calendar_max_duration_mins >= 60 && prev.calendar_max_duration_mins < 60 {
        events.push(HwEvent::LongMeetingDetected);
    }
    if current.calendar_back_to_back && !prev.calendar_back_to_back {
        events.push(HwEvent::BackToBackMeetings);
    }
    if current.focused_ui_sig != prev.focused_ui_sig
        && current.focused_ui_sig.is_some()
        && prev.focused_ui_sig.is_some()
    {
        events.push(HwEvent::FocusedUiElementChanged);
    }
    if current.focused_ui_value_sig != prev.focused_ui_value_sig
        && current.focused_ui_value_sig.is_some()
        && prev.focused_ui_value_sig.is_some()
    {
        events.push(HwEvent::UiElementValueChanged);
    }
    if current.selected_text_sig != prev.selected_text_sig
        && current.selected_text_sig.is_some()
        && prev.selected_text_sig.is_some()
    {
        events.push(HwEvent::SelectedTextChanged);
    }
    if current.scroll_position_sig != prev.scroll_position_sig
        && current.scroll_position_sig.is_some()
        && prev.scroll_position_sig.is_some()
    {
        events.push(HwEvent::ScrollPositionChanged);
    }
    if current.accessibility_enabled && !prev.accessibility_enabled {
        events.push(HwEvent::AccessibilityEnabled);
    }
    if current.reduce_motion_enabled != prev.reduce_motion_enabled {
        events.push(HwEvent::ReduceMotionToggled);
    }
    if current.increase_contrast_enabled != prev.increase_contrast_enabled {
        events.push(HwEvent::IncreaseContrastToggled);
    }
    if current.voiceover_enabled && !prev.voiceover_enabled {
        events.push(HwEvent::VoiceOverEnabled);
    }
    if current.location_sig != prev.location_sig
        && current.location_sig.is_some()
        && prev.location_sig.is_some()
    {
        events.push(HwEvent::LocationUpdated);
    }
    if let (Some(clat), Some(clon), Some(plat), Some(plon)) = (
        current.location_lat,
        current.location_lon,
        prev.location_lat,
        prev.location_lon,
    ) {
        if haversine_km(plat, plon, clat, clon) >= 50.0 {
            events.push(HwEvent::SignificantLocationChange);
        }
    }
    if current.daylight != prev.daylight && current.daylight.is_some() && prev.daylight.is_some() {
        events.push(HwEvent::SunriseSunset);
    }
    if current.weather_condition_sig != prev.weather_condition_sig
        && current.weather_condition_sig.is_some()
        && prev.weather_condition_sig.is_some()
    {
        events.push(HwEvent::LocalWeatherCondition);
    }
    if clock_jumped(prev.ts, current.ts, observe_tick) {
        events.push(HwEvent::SystemClockJumped);
    }
    if current.process_count.abs_diff(prev.process_count) >= 5 {
        events.push(HwEvent::ProcessListChanged);
    }
    if current.top_process != prev.top_process
        && current.top_process.is_some()
        && prev.top_process.is_some()
    {
        events.push(HwEvent::TopProcessChanged);
    }
    if current.packet_loss_count.is_some_and(|cur| {
        let old = prev.packet_loss_count.unwrap_or(0);
        cur > old && cur.saturating_sub(old) >= 10
    }) {
        events.push(HwEvent::NetworkPacketLossSpike);
    }
    if current.kernel_panic_sig != prev.kernel_panic_sig && current.kernel_panic_sig.is_some() {
        events.push(HwEvent::KernelPanicDetected);
    }
    if current.vpn_active && !prev.vpn_active {
        events.push(HwEvent::VpnConnected);
    }
    if !current.vpn_active && prev.vpn_active {
        events.push(HwEvent::VpnDisconnected);
    }
    if current.media_playing && !prev.media_playing {
        events.push(HwEvent::MediaStarted);
    }
    if !current.media_playing && prev.media_playing {
        events.push(HwEvent::MediaStopped);
    }
    if current.now_playing_track != prev.now_playing_track
        && current.now_playing_track.is_some()
        && prev.now_playing_track.is_some()
        && current.now_playing_app == prev.now_playing_app
    {
        events.push(HwEvent::MediaTrackChanged);
    }
    if let (Some(cur), Some(old)) = (current.output_volume_pct, prev.output_volume_pct)
        && (cur - old).abs() >= 1.0
    {
        events.push(HwEvent::SystemVolumeChanged);
    }
    if current.output_muted && !prev.output_muted {
        events.push(HwEvent::SystemMuted);
    }
    if current.mic_active && !prev.mic_active {
        events.push(HwEvent::MicrophoneActivated);
    }
    if !current.mic_active && prev.mic_active {
        events.push(HwEvent::MicrophoneDeactivated);
    }
    if current.audio_output_device != prev.audio_output_device
        && current.audio_output_device.is_some()
        && prev.audio_output_device.is_some()
    {
        events.push(HwEvent::AudioOutputDeviceChanged);
    }
    if current.headphones_connected && !prev.headphones_connected {
        events.push(HwEvent::HeadphonesConnected);
    }
    if !current.headphones_connected && prev.headphones_connected {
        events.push(HwEvent::HeadphonesDisconnected);
    }
    if let (Some(cur), Some(old)) = (current.input_volume_pct, prev.input_volume_pct)
        && (cur - old).abs() >= 1.0
    {
        events.push(HwEvent::AudioInputLevelChanged);
    }
    if current.system_alert_sound_active && !prev.system_alert_sound_active {
        events.push(HwEvent::SystemAlertSoundPlayed);
    }
    if current.airplay_active && !prev.airplay_active {
        events.push(HwEvent::AirPlaySessionStarted);
    }
    if current.now_playing_app != prev.now_playing_app
        && current.now_playing_app.is_some()
        && prev.now_playing_app.is_some()
    {
        events.push(HwEvent::NowPlayingAppChanged);
    }

    events
}

fn sleep_imminent(snapshot: &observe::snapshot::OsSnapshot) -> bool {
    let Some(display_sleep_mins) = snapshot.display_sleep_minutes else {
        return false;
    };
    if snapshot.sleep_prevented || snapshot.screen_locked {
        return false;
    }
    let target = display_sleep_mins.saturating_mul(60) as i64;
    let idle = snapshot.idle_secs as i64;
    (target - idle).abs() <= 20
}

fn normalized_console_user(user: Option<&str>) -> Option<&str> {
    let u = user?.trim();
    if u.is_empty() {
        return None;
    }
    let l = u.to_ascii_lowercase();
    if matches!(l.as_str(), "root" | "loginwindow" | "_mbsetupuser") {
        None
    } else {
        Some(u)
    }
}

fn clock_jumped(prev_ts: u64, cur_ts: u64, observe_tick: Duration) -> bool {
    if cur_ts + 5 < prev_ts {
        return true;
    }
    let delta = cur_ts.saturating_sub(prev_ts);
    let expected = observe_tick.as_secs();
    let diff = delta.abs_diff(expected);
    // Ignore normal jitter and explicit sleep/wake gaps; catch true wall-clock shifts.
    diff >= 120 && delta < 3600
}

fn haversine_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let r = 6371.0_f64;
    let dlat = (lat2 - lat1).to_radians();
    let dlon = (lon2 - lon1).to_radians();
    let a = (dlat / 2.0).sin().powi(2)
        + lat1.to_radians().cos() * lat2.to_radians().cos() * (dlon / 2.0).sin().powi(2);
    let c = 2.0 * a.sqrt().atan2((1.0 - a).sqrt());
    r * c
}

fn looks_fullscreen(snapshot: &observe::snapshot::OsSnapshot) -> bool {
    let Some(geom) = snapshot.active_window_geom_sig.as_deref() else {
        return false;
    };
    let Some((_, _, ww, wh)) = parse_window_geom(geom) else {
        return false;
    };
    let Some(res_sig) = snapshot.display_resolution_sig.as_deref() else {
        return false;
    };
    let Some((dw, dh)) = parse_display_res(res_sig) else {
        return false;
    };
    ww as f32 >= dw as f32 * 0.95 && wh as f32 >= dh as f32 * 0.90
}

fn parse_window_geom(s: &str) -> Option<(i32, i32, i32, i32)> {
    // format: x,y,wxh
    let (xy, wh) = s.split_once(',')?;
    let x = xy.parse::<i32>().ok()?;
    let (y_s, wh_s) = wh.split_once(',')?;
    let y = y_s.parse::<i32>().ok()?;
    let (w_s, h_s) = wh_s.split_once('x')?;
    let w = w_s.parse::<i32>().ok()?;
    let h = h_s.parse::<i32>().ok()?;
    Some((x, y, w, h))
}

fn parse_display_res(sig: &str) -> Option<(i32, i32)> {
    // example piece: "Resolution: 2560 x 1600 Retina"
    let t = sig.split('|').next()?.trim();
    let nums: Vec<i32> = t
        .chars()
        .map(|c| if c.is_ascii_digit() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .filter_map(|p| p.parse::<i32>().ok())
        .collect();
    if nums.len() >= 2 {
        Some((nums[0], nums[1]))
    } else {
        None
    }
}

fn is_likely_pinch(prev_geom: Option<&str>, cur_geom: Option<&str>) -> bool {
    let Some((_, _, pw, ph)) = prev_geom.and_then(parse_window_geom) else {
        return false;
    };
    let Some((_, _, cw, ch)) = cur_geom.and_then(parse_window_geom) else {
        return false;
    };
    if pw <= 0 || ph <= 0 || cw <= 0 || ch <= 0 {
        return false;
    }
    let prev_area = (pw as f64) * (ph as f64);
    let cur_area = (cw as f64) * (ch as f64);
    let ratio = (cur_area / prev_area).max(prev_area / cur_area);
    ratio >= 1.20
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::{core::runtime::HwEvent, observe::snapshot::OsSnapshot};

    use super::detect_hw_events;

    fn snapshot_with_ts(ts: u64) -> OsSnapshot {
        OsSnapshot {
            ts,
            ..OsSnapshot::default()
        }
    }

    #[test]
    fn battery_low_triggers_once_on_threshold_crossing() {
        let tick = Duration::from_secs(2);
        let mut prev = snapshot_with_ts(100);
        prev.battery_pct = Some(30.0);

        let mut current = snapshot_with_ts(102);
        current.battery_pct = Some(19.0);

        let events = detect_hw_events(Some(&prev), &current, tick);
        assert!(events.contains(&HwEvent::BatteryLow));

        let mut next = snapshot_with_ts(104);
        next.battery_pct = Some(18.0);
        let next_events = detect_hw_events(Some(&current), &next, tick);
        assert!(!next_events.contains(&HwEvent::BatteryLow));
    }

    #[test]
    fn charger_plugged_triggers_once_until_state_changes_again() {
        let tick = Duration::from_secs(2);
        let mut prev = snapshot_with_ts(200);
        prev.charging = false;

        let mut current = snapshot_with_ts(202);
        current.charging = true;

        let events = detect_hw_events(Some(&prev), &current, tick);
        assert!(events.contains(&HwEvent::ChargerPluggedIn));
        assert!(events.contains(&HwEvent::PowerSourceChanged));

        let mut next = snapshot_with_ts(204);
        next.charging = true;
        let next_events = detect_hw_events(Some(&current), &next, tick);
        assert!(!next_events.contains(&HwEvent::ChargerPluggedIn));
        assert!(!next_events.contains(&HwEvent::PowerSourceChanged));
    }
}
