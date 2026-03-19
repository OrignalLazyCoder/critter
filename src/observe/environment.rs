use std::{
    process::Command,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use chrono::{Local, Timelike};

use super::snapshot::OsSnapshot;

#[derive(Debug, Clone, Default)]
struct EnvironmentState {
    location_sig: Option<String>,
    location_lat: Option<f64>,
    location_lon: Option<f64>,
    weather_condition_sig: Option<String>,
    sunrise_sig: Option<String>,
    sunset_sig: Option<String>,
    daylight: Option<bool>,
}

static CACHE: OnceLock<Mutex<Option<(Instant, EnvironmentState)>>> = OnceLock::new();

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
            && ts.elapsed() < Duration::from_secs(300)
        {
            apply_state(snapshot, st.clone());
            return;
        }

        let st = query_environment().unwrap_or_default();
        apply_state(snapshot, st.clone());
        *guard = Some((Instant::now(), st));
    }
}

#[cfg(target_os = "macos")]
fn apply_state(snapshot: &mut OsSnapshot, st: EnvironmentState) {
    snapshot.location_sig = st.location_sig;
    snapshot.location_lat = st.location_lat;
    snapshot.location_lon = st.location_lon;
    snapshot.weather_condition_sig = st.weather_condition_sig;
    snapshot.sunrise_sig = st.sunrise_sig;
    snapshot.sunset_sig = st.sunset_sig;
    snapshot.daylight = st.daylight;
}

#[cfg(target_os = "macos")]
fn query_environment() -> Option<EnvironmentState> {
    let py = r#"import json, urllib.request
url = "https://wttr.in/?format=j1"
with urllib.request.urlopen(url, timeout=2) as r:
    data = json.load(r)
area = (data.get("nearest_area") or [{}])[0]
city = ((area.get("areaName") or [{}])[0].get("value") or "").strip()
region = ((area.get("region") or [{}])[0].get("value") or "").strip()
country = ((area.get("country") or [{}])[0].get("value") or "").strip()
lat = (area.get("latitude") or "").strip()
lon = (area.get("longitude") or "").strip()
cc = (data.get("current_condition") or [{}])[0]
weather = (((cc.get("weatherDesc") or [{}])[0].get("value")) or "").strip()
astr = (((data.get("weather") or [{}])[0].get("astronomy") or [{}])[0]
sunrise = (astr.get("sunrise") or "").strip()
sunset = (astr.get("sunset") or "").strip()
print("|".join([city, region, country, lat, lon, weather, sunrise, sunset]))"#;

    let out = run_cmd("python3", &["-c", py])?;
    let line = out.lines().next()?.trim();
    if line.is_empty() {
        return None;
    }
    let mut p = line.splitn(8, '|');
    let city = p.next().unwrap_or_default().trim();
    let region = p.next().unwrap_or_default().trim();
    let country = p.next().unwrap_or_default().trim();
    let lat = p.next().unwrap_or_default().trim();
    let lon = p.next().unwrap_or_default().trim();
    let weather = p.next().unwrap_or_default().trim();
    let sunrise = p.next().unwrap_or_default().trim();
    let sunset = p.next().unwrap_or_default().trim();

    let mut st = EnvironmentState::default();
    let loc = [city, region, country]
        .iter()
        .copied()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    if !loc.is_empty() {
        st.location_sig = Some(loc);
    }
    st.location_lat = lat.parse::<f64>().ok();
    st.location_lon = lon.parse::<f64>().ok();
    if !weather.is_empty() {
        st.weather_condition_sig = Some(weather.to_string());
    }
    if !sunrise.is_empty() {
        st.sunrise_sig = Some(sunrise.to_string());
    }
    if !sunset.is_empty() {
        st.sunset_sig = Some(sunset.to_string());
    }

    if let (Some(sr), Some(ss)) = (
        parse_ampm_minutes(st.sunrise_sig.as_deref().unwrap_or_default()),
        parse_ampm_minutes(st.sunset_sig.as_deref().unwrap_or_default()),
    ) {
        let now = Local::now();
        let cur = (now.hour() * 60 + now.minute()) as u16;
        st.daylight = Some(cur >= sr && cur < ss);
    }

    Some(st)
}

#[cfg(target_os = "macos")]
fn parse_ampm_minutes(s: &str) -> Option<u16> {
    let t = s.trim().to_ascii_uppercase();
    let (time, meridiem) = if let Some((a, b)) = t.split_once(' ') {
        (a.trim(), b.trim())
    } else {
        return None;
    };
    let (h, m) = time.split_once(':')?;
    let mut hour = h.parse::<u16>().ok()?;
    let min = m.parse::<u16>().ok()?;
    if min >= 60 || hour == 0 || hour > 12 {
        return None;
    }
    match meridiem {
        "AM" => {
            if hour == 12 {
                hour = 0;
            }
        }
        "PM" => {
            if hour != 12 {
                hour += 12;
            }
        }
        _ => return None,
    }
    Some(hour * 60 + min)
}

fn run_cmd(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
}
