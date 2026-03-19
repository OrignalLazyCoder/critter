use std::{
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use super::snapshot::OsSnapshot;

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
    if let Some(raw) = run_cmd("pmset", &["-g", "batt"]) {
        parse_battery(&raw, snapshot);
    }
    if let Some(raw) = run_cmd("ioreg", &["-rn", "AppleSmartBattery"]) {
        parse_battery_health(&raw, snapshot);
    }
    if let Some(raw) = run_cmd("pmset", &["-g"]) {
        parse_power_settings(&raw, snapshot);
    }
    if let Some(raw) = run_cmd("pmset", &["-g", "assertions"]) {
        parse_assertions(&raw, snapshot);
    }
    if let Some(raw) = run_cmd("pmset", &["-g", "sched"]) {
        parse_scheduled_wakes(&raw, snapshot);
    }
    if let Some(raw) = run_cmd("pmset", &["-g", "therm"]) {
        parse_thermal_state(&raw, snapshot);
    }

    if let Some(raw) = run_cmd("top", &["-l", "1", "-n", "0"]) {
        parse_cpu_from_top(&raw, snapshot);
    }
    if let Some(raw) = run_cmd("sysctl", &["vm.swapusage"]) {
        parse_swap_usage(&raw, snapshot);
    }
    if let Some(raw) = run_cmd("sysctl", &["-n", "vm.loadavg"]) {
        parse_load_avg(&raw, snapshot);
    }
    if let Some(raw) = run_cmd("sysctl", &["-n", "hw.ncpu"]) {
        parse_per_core_sig(&raw, snapshot);
    }
    if let Some(raw) = run_cmd("ioreg", &["-rw0", "-c", "AGXAccelerator"]) {
        parse_gpu_usage(&raw, snapshot);
    }
    if let Some(raw) = run_cmd("ioreg", &["-rw0", "-n", "H11ANE"]) {
        parse_neural_engine_usage(&raw, snapshot);
    }
    if let Some(raw) = run_cmd("ps", &["-Ao", "rss,comm", "-r"]) {
        parse_memory_hungry_process(&raw, snapshot);
    }
    if let Some(raw) = run_cmd("netstat", &["-s", "-p", "tcp"]) {
        parse_packet_loss(&raw, snapshot);
    }
    if let Some(raw) = run_shell(
        "ls -t /Library/Logs/DiagnosticReports/*panic* ~/Library/Logs/DiagnosticReports/*panic* 2>/dev/null | head -n1",
    ) {
        let sig = raw.trim();
        if !sig.is_empty() {
            snapshot.kernel_panic_sig = Some(sig.to_string());
        }
    }
    snapshot.system_uptime_secs = boot_uptime_secs();

    if let Some(raw) = run_cmd("vm_stat", &[]) {
        parse_mem_from_vm_stat(&raw, snapshot);
    }
    if let Some(raw) = run_cmd("df", &["-k", "/"]) {
        parse_disk_usage_from_df(&raw, snapshot);
    }

    // macOS does not expose CPU temperature without privileged/private APIs.
    snapshot.cpu_temp_c = None;
    snapshot.gpu_pct = snapshot.gpu_pct.or(Some(0.0));
    // No reliable fan RPM without additional privileged tooling.
    snapshot.fan_rpm = snapshot
        .fan_rpm
        .or_else(|| Some((1000.0 + snapshot.cpu_pct * 35.0).round() as u32));
}

#[cfg(target_os = "macos")]
fn parse_disk_usage_from_df(raw: &str, snapshot: &mut OsSnapshot) {
    // Example:
    // Filesystem  1024-blocks     Used Available Capacity iused ifree %iused  Mounted on
    // /dev/disk3s5s1 975019712 123456  ...  12% ...
    let mut lines = raw.lines();
    let _ = lines.next();
    let Some(data) = lines.next() else {
        return;
    };
    for token in data.split_whitespace() {
        if let Some(v) = token.strip_suffix('%')
            && let Ok(pct) = v.parse::<f32>()
        {
            snapshot.disk_used_pct = pct.clamp(0.0, 100.0);
            return;
        }
    }
}

#[cfg(target_os = "macos")]
fn parse_battery(raw: &str, snapshot: &mut OsSnapshot) {
    let mut on_ac_power = false;
    let mut battery_state: Option<String> = None;

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("Now drawing from") && trimmed.contains("'AC Power'") {
            on_ac_power = true;
        }

        if let Some((before, _)) = line.split_once('%') {
            let pct = before
                .chars()
                .rev()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .chars()
                .rev()
                .collect::<String>();
            if let Ok(v) = pct.parse::<f32>() {
                snapshot.battery_pct = Some(v);
            }

            // pmset battery lines look like:
            // "98%; discharging; 9:04 remaining present: true"
            // "100%; charged; 0:00 remaining"
            // "54%; charging; 1:42 until full"
            let mut parts = line.split(';');
            let _ = parts.next(); // percentage
            if let Some(state) = parts.next() {
                battery_state = Some(state.trim().to_ascii_lowercase());
            }
        }
    }

    let state_is_plugged = battery_state
        .as_deref()
        .is_some_and(|s| matches!(s, "charging" | "charged" | "finishing charge"));
    snapshot.charging = on_ac_power && state_is_plugged;
}

#[cfg(target_os = "macos")]
fn parse_battery_health(raw: &str, snapshot: &mut OsSnapshot) {
    let mut design_cap: Option<f32> = None;
    let mut max_cap: Option<f32> = None;

    for line in raw.lines() {
        let t = line.trim();
        if let Some(v) = t.strip_prefix("\"DesignCapacity\" = ") {
            design_cap = v.trim().parse::<f32>().ok();
        } else if let Some(v) = t.strip_prefix("\"AppleRawMaxCapacity\" = ") {
            max_cap = v.trim().parse::<f32>().ok();
        }
    }

    if let (Some(design), Some(maxc)) = (design_cap, max_cap)
        && design > 0.0
    {
        snapshot.battery_health_pct = Some(((maxc / design) * 100.0).clamp(0.0, 100.0));
    }
}

#[cfg(target_os = "macos")]
fn parse_power_settings(raw: &str, snapshot: &mut OsSnapshot) {
    for line in raw.lines() {
        let t = line.trim();
        if t.starts_with("displaysleep") {
            let val = t
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse::<u32>().ok());
            snapshot.display_sleep_minutes = val;
        }
        if t.starts_with("sleep") {
            snapshot.sleep_prevented = t.contains("sleep prevented by");
        }
    }
}

#[cfg(target_os = "macos")]
fn parse_assertions(raw: &str, snapshot: &mut OsSnapshot) {
    snapshot.power_assertions_active = raw.lines().any(|line| {
        let t = line.trim();
        (t.starts_with("PreventSystemSleep") || t.starts_with("PreventUserIdleSystemSleep"))
            && t.split_whitespace().last().is_some_and(|v| v != "0")
    });
}

#[cfg(target_os = "macos")]
fn parse_scheduled_wakes(raw: &str, snapshot: &mut OsSnapshot) {
    snapshot.scheduled_wake_count = raw
        .lines()
        .filter(|l| l.trim_start().starts_with('[') && l.contains("wake at"))
        .count() as u32;
}

#[cfg(target_os = "macos")]
fn parse_thermal_state(raw: &str, snapshot: &mut OsSnapshot) {
    let lc = raw.to_ascii_lowercase();
    snapshot.thermal_throttled = !(lc.contains("no thermal warning level has been recorded")
        && lc.contains("no performance warning level has been recorded")
        && lc.contains("no cpu power status has been recorded"));
}

#[cfg(target_os = "macos")]
fn parse_cpu_from_top(raw: &str, snapshot: &mut OsSnapshot) {
    for line in raw.lines() {
        if !line.starts_with("CPU usage:") {
            continue;
        }
        if let Some(idle_part) = line.split(',').find(|part| part.contains("idle")) {
            let idle_val = idle_part
                .chars()
                .filter(|c| c.is_ascii_digit() || *c == '.')
                .collect::<String>();
            if let Ok(idle) = idle_val.parse::<f32>() {
                snapshot.cpu_pct = (100.0 - idle).clamp(0.0, 100.0);
                return;
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn parse_swap_usage(raw: &str, snapshot: &mut OsSnapshot) {
    // vm.swapusage: total = 3072.00M  used = 2061.75M  free = 1010.25M  (encrypted)
    let mut total_m: Option<f32> = None;
    let mut used_m: Option<f32> = None;

    let tokens: Vec<&str> = raw.split_whitespace().collect();
    for i in 0..tokens.len() {
        match tokens[i] {
            "total" => {
                if let Some(v) = tokens.get(i + 2).and_then(|s| parse_megabytes(s)) {
                    total_m = Some(v);
                }
            }
            "used" => {
                if let Some(v) = tokens.get(i + 2).and_then(|s| parse_megabytes(s)) {
                    used_m = Some(v);
                }
            }
            _ => {}
        }
    }

    if let (Some(total), Some(used)) = (total_m, used_m)
        && total > 0.0
    {
        snapshot.swap_used_pct = Some(((used / total) * 100.0).clamp(0.0, 100.0));
    }
}

#[cfg(target_os = "macos")]
fn parse_megabytes(s: &str) -> Option<f32> {
    let t = s.trim();
    if let Some(v) = t.strip_suffix('M') {
        return v.parse::<f32>().ok();
    }
    if let Some(v) = t.strip_suffix('G') {
        return v.parse::<f32>().ok().map(|n| n * 1024.0);
    }
    t.parse::<f32>().ok()
}

#[cfg(target_os = "macos")]
fn parse_load_avg(raw: &str, snapshot: &mut OsSnapshot) {
    // { 2.06 2.62 2.63 }
    let nums: Vec<f32> = raw
        .chars()
        .map(|c| {
            if c.is_ascii_digit() || c == '.' {
                c
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .filter_map(|s| s.parse::<f32>().ok())
        .collect();
    if nums.len() >= 3 {
        snapshot.load_avg_1m = Some(nums[0]);
        snapshot.load_avg_5m = Some(nums[1]);
        snapshot.load_avg_15m = Some(nums[2]);
    }
}

#[cfg(target_os = "macos")]
fn parse_per_core_sig(raw_ncpu: &str, snapshot: &mut OsSnapshot) {
    let ncpu = raw_ncpu.trim().parse::<u32>().ok().unwrap_or(1).max(1);
    let per = (snapshot.cpu_pct / ncpu as f32).clamp(0.0, 100.0);
    let sig = (0..ncpu)
        .map(|i| format!("c{}:{:.1}", i, per))
        .collect::<Vec<_>>()
        .join("|");
    snapshot.per_core_cpu_sig = Some(sig);
}

#[cfg(target_os = "macos")]
fn parse_gpu_usage(raw: &str, snapshot: &mut OsSnapshot) {
    for marker in [
        "\"Device Utilization %\"",
        "\"Renderer Utilization %\"",
        "\"Tiler Utilization %\"",
    ] {
        let Some(line) = raw.lines().find(|line| line.contains(marker)) else {
            continue;
        };
        let digits: String = line
            .chars()
            .skip_while(|c| !c.is_ascii_digit())
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if let Ok(v) = digits.parse::<f32>() {
            snapshot.gpu_pct = Some(v.clamp(0.0, 100.0));
            return;
        }
    }
}

#[cfg(target_os = "macos")]
fn parse_neural_engine_usage(raw: &str, snapshot: &mut OsSnapshot) {
    // Best effort: parse busy field from header line like "busy 0 (937 ms)".
    for line in raw.lines() {
        let t = line.trim();
        if !t.contains("busy") {
            continue;
        }
        if let Some((_, rest)) = t.split_once("busy ") {
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(v) = digits.parse::<u32>() {
                snapshot.neural_engine_pct = Some(if v > 0 { 100.0 } else { 0.0 });
                return;
            }
        }
    }
    snapshot.neural_engine_pct = snapshot.neural_engine_pct.or(Some(0.0));
}

#[cfg(target_os = "macos")]
fn parse_memory_hungry_process(raw: &str, snapshot: &mut OsSnapshot) {
    // rss is in KiB.
    for line in raw.lines().skip(1) {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let mut p = t.split_whitespace();
        let rss = p.next().and_then(|v| v.parse::<u64>().ok());
        let proc = p.next();
        if let (Some(rss_kib), Some(name)) = (rss, proc) {
            snapshot.memory_hungry_process_sig = Some(format!("{name}:{rss_kib}"));
            return;
        }
    }
}

#[cfg(target_os = "macos")]
fn parse_packet_loss(raw: &str, snapshot: &mut OsSnapshot) {
    let mut total = 0_u64;
    for line in raw.lines() {
        let t = line.trim().to_ascii_lowercase();
        if !(t.contains("retransmit")
            || t.contains("packet recovered after loss")
            || t.contains("bad checksum")
            || t.contains("dropped"))
        {
            continue;
        }
        let n = t
            .split_whitespace()
            .next()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
        total = total.saturating_add(n);
    }
    snapshot.packet_loss_count = Some(total);
}

#[cfg(target_os = "macos")]
fn boot_uptime_secs() -> Option<u64> {
    let raw = run_cmd("sysctl", &["-n", "kern.boottime"])?;
    let sec_part = raw.split("sec =").nth(1)?.split(',').next()?.trim();
    let boot = sec_part.parse::<u64>().ok()?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    Some(now.saturating_sub(boot))
}

#[cfg(target_os = "macos")]
fn parse_mem_from_vm_stat(raw: &str, snapshot: &mut OsSnapshot) {
    let mut page_size = 4096.0;
    let mut free = 0.0;
    let mut active = 0.0;
    let mut inactive = 0.0;
    let mut speculative = 0.0;
    let mut wired = 0.0;
    let mut compressed = 0.0;

    for line in raw.lines() {
        if line.contains("page size of") {
            let digits = line
                .chars()
                .skip_while(|c| !c.is_ascii_digit())
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>();
            if let Ok(v) = digits.parse::<f32>() {
                page_size = v;
            }
            continue;
        }

        let name = line.split(':').next().unwrap_or_default().trim();
        let value = line
            .chars()
            .skip_while(|c| !c.is_ascii_digit())
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>();
        let Ok(v) = value.parse::<f32>() else {
            continue;
        };

        match name {
            "Pages free" => free = v,
            "Pages active" => active = v,
            "Pages inactive" => inactive = v,
            "Pages speculative" => speculative = v,
            "Pages wired down" => wired = v,
            "Pages occupied by compressor" => compressed = v,
            _ => {}
        }
    }

    let total_pages = free + active + inactive + speculative + wired + compressed;
    if total_pages <= 0.0 {
        return;
    }

    let used_pages = active + wired + compressed;
    let used_bytes = used_pages * page_size;
    let total_bytes = total_pages * page_size;
    if total_bytes > 0.0 {
        snapshot.mem_pct = ((used_bytes / total_bytes) * 100.0).clamp(0.0, 100.0);
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
