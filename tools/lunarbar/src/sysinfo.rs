//! System metrics for lunarbar, read straight from /proc and libc — no external
//! crates. cpu% (busy delta from /proc/stat), mem% (/proc/meminfo), and the
//! wall clock (libc localtime, UTC when no TZ is set).

/// Rolling CPU-usage sampler. `/proc/stat`'s aggregate `cpu` line gives
/// cumulative jiffies; usage is the busy fraction of the delta between reads.
#[derive(Default)]
pub struct CpuMeter {
    prev_busy: u64,
    prev_total: u64,
    have_prev: bool,
}

impl CpuMeter {
    /// Returns integer CPU-busy percent since the previous call, or `None`
    /// on the first call / if /proc/stat is unreadable.
    pub fn sample(&mut self) -> Option<u32> {
        let stat = std::fs::read_to_string("/proc/stat").ok()?;
        let line = stat.lines().next()?; // "cpu  u n s idle iowait irq softirq steal ..."
        if !line.starts_with("cpu ") && !line.starts_with("cpu\t") {
            return None;
        }
        let vals: Vec<u64> = line
            .split_whitespace()
            .skip(1)
            .filter_map(|v| v.parse().ok())
            .collect();
        if vals.len() < 4 {
            return None;
        }
        let idle = vals[3] + vals.get(4).copied().unwrap_or(0); // idle + iowait
        let total: u64 = vals.iter().sum();
        let busy = total.saturating_sub(idle);

        let out = if self.have_prev {
            let dt = total.saturating_sub(self.prev_total);
            let db = busy.saturating_sub(self.prev_busy);
            if dt == 0 {
                Some(0)
            } else {
                Some(((db * 100) / dt).min(100) as u32)
            }
        } else {
            None
        };
        self.prev_busy = busy;
        self.prev_total = total;
        self.have_prev = true;
        out
    }
}

/// Memory-used percent from /proc/meminfo: 100 * (MemTotal - MemAvailable) /
/// MemTotal. Falls back to MemFree when MemAvailable is absent.
pub fn mem_percent() -> Option<u32> {
    let mi = std::fs::read_to_string("/proc/meminfo").ok()?;
    let mut total = 0u64;
    let mut avail = 0u64;
    let mut free = 0u64;
    for line in mi.lines() {
        let mut it = line.split_whitespace();
        let key = it.next().unwrap_or("");
        let val: u64 = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        match key {
            "MemTotal:" => total = val,
            "MemAvailable:" => avail = val,
            "MemFree:" => free = val,
            _ => {}
        }
    }
    if total == 0 {
        return None;
    }
    let available = if avail > 0 { avail } else { free };
    let used = total.saturating_sub(available);
    Some(((used * 100) / total).min(100) as u32)
}

/// Wall clock "HH:MM" (24h). Uses libc localtime_r so a set TZ is honoured;
/// with no TZ, musl returns UTC — fine for a bar clock.
pub fn clock_hhmm() -> String {
    unsafe {
        // Infer the platform time type — naming `libc::time_t` is deprecated
        // (musl 1.2.0 moves it to 64-bit; see rust-lang/libc#1848).
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = core::mem::zeroed();
        // localtime_r is thread-safe and never allocates a static buffer.
        if libc::localtime_r(&t, &mut tm).is_null() {
            return "--:--".into();
        }
        format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
    }
}

/// Monotonic seconds (CLOCK_MONOTONIC) as f64 — used to rate-scale network
/// counters independently of how often the render loop actually fires.
fn mono_secs() -> f64 {
    unsafe {
        let mut ts: libc::timespec = core::mem::zeroed();
        if libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) != 0 {
            return 0.0;
        }
        ts.tv_sec as f64 + ts.tv_nsec as f64 / 1e9
    }
}

/// Network throughput sampler. Sums rx/tx byte counters across every non-
/// loopback interface in /proc/net/dev and divides the delta by real elapsed
/// (monotonic) time, so the rate is correct even if repaints are irregular.
#[derive(Default)]
pub struct NetMeter {
    prev_rx: u64,
    prev_tx: u64,
    prev_t: f64,
    have_prev: bool,
}

/// A single network sample: down/up bytes-per-second and whether any real
/// interface is administratively up.
#[derive(Clone, Copy)]
pub struct NetRate {
    pub down: f64,
    pub up: f64,
    pub link: bool,
}

impl NetMeter {
    pub fn sample(&mut self) -> Option<NetRate> {
        let dev = std::fs::read_to_string("/proc/net/dev").ok()?;
        let (mut rx, mut tx) = (0u64, 0u64);
        for line in dev.lines().skip(2) {
            let Some((name, rest)) = line.split_once(':') else {
                continue;
            };
            let name = name.trim();
            if name == "lo" {
                continue;
            }
            let f: Vec<u64> = rest.split_whitespace().filter_map(|v| v.parse().ok()).collect();
            // /proc/net/dev columns: rx_bytes=0 … tx_bytes=8
            if f.len() >= 9 {
                rx += f[0];
                tx += f[8];
            }
        }
        let now = mono_secs();
        let out = if self.have_prev {
            let dt = (now - self.prev_t).max(1e-3);
            NetRate {
                down: rx.saturating_sub(self.prev_rx) as f64 / dt,
                up: tx.saturating_sub(self.prev_tx) as f64 / dt,
                link: net_link_up(),
            }
        } else {
            NetRate { down: 0.0, up: 0.0, link: net_link_up() }
        };
        self.prev_rx = rx;
        self.prev_tx = tx;
        self.prev_t = now;
        self.have_prev = true;
        Some(out)
    }
}

/// True if any non-loopback interface reports operstate "up".
fn net_link_up() -> bool {
    let Ok(dir) = std::fs::read_dir("/sys/class/net") else {
        return false;
    };
    for e in dir.flatten() {
        let name = e.file_name();
        if name == "lo" {
            continue;
        }
        let p = e.path().join("operstate");
        if let Ok(s) = std::fs::read_to_string(&p) {
            if s.trim() == "up" {
                return true;
            }
        }
    }
    false
}

/// Human-readable byte-rate, e.g. "1.2M", "834K", "12B". Kept to ≤4 chars so
/// the module width stays stable even for absurd rates.
pub fn fmt_rate(bps: f64) -> String {
    let s = if bps >= 1_000_000_000.0 {
        format!("{:.0}G", bps / 1_073_741_824.0)
    } else if bps >= 1_000_000.0 {
        format!("{:.1}M", bps / 1_048_576.0)
    } else if bps >= 1_000.0 {
        format!("{:.0}K", bps / 1024.0)
    } else {
        format!("{:.0}B", bps)
    };
    if s.len() <= 4 {
        s
    } else {
        // Truncate visually rather than shove neighbouring modules off-screen.
        s.chars().take(4).collect()
    }
}

/// Uptime as a compact "Nd Nh", "Nh Nm" or "Nm" string from /proc/uptime.
pub fn uptime() -> Option<String> {
    let s = std::fs::read_to_string("/proc/uptime").ok()?;
    let secs: f64 = s.split_whitespace().next()?.parse().ok()?;
    let secs = secs as u64;
    let (d, h, m) = (secs / 86400, (secs % 86400) / 3600, (secs % 3600) / 60);
    Some(if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m")
    })
}

/// 1-minute load average from /proc/loadavg.
pub fn loadavg() -> Option<f32> {
    let s = std::fs::read_to_string("/proc/loadavg").ok()?;
    s.split_whitespace().next()?.parse().ok()
}

/// Root-filesystem used percent via statvfs("/").
pub fn disk_root_percent() -> Option<u32> {
    unsafe {
        let mut st: libc::statvfs = core::mem::zeroed();
        let path = b"/\0";
        if libc::statvfs(path.as_ptr() as *const libc::c_char, &mut st) != 0 {
            return None;
        }
        let total = st.f_blocks as u64;
        if total == 0 {
            return None;
        }
        let avail = st.f_bavail as u64;
        let used = total.saturating_sub(avail);
        Some(((used * 100) / total).min(100) as u32)
    }
}

/// CPU temperature in whole °C from a CPU/x86-package thermal zone when
/// typed as such, else the first readable zone (absent → None, module hidden).
pub fn temp_c() -> Option<u32> {
    let mut fallback = None;
    for zone in 0..8 {
        let base = format!("/sys/class/thermal/thermal_zone{zone}");
        let typed = std::fs::read_to_string(format!("{base}/type"))
            .map(|t| {
                let t = t.trim().to_ascii_lowercase();
                t.contains("cpu") || t.contains("x86") || t.contains("pkg") || t.contains("core")
            })
            .unwrap_or(false);
        if let Ok(milli) = std::fs::read_to_string(format!("{base}/temp")) {
            if let Ok(m) = milli.trim().parse::<i64>() {
                if m > 0 {
                    let c = (m / 1000).clamp(0, 200) as u32;
                    if typed {
                        return Some(c);
                    }
                    if fallback.is_none() {
                        fallback = Some(c);
                    }
                }
            }
        }
    }
    fallback
}

/// Battery percent + charging flag from /sys/class/power_supply, if any
/// battery is present (desktops → None, module hidden).
pub fn battery() -> Option<(u32, bool)> {
    let dir = std::fs::read_dir("/sys/class/power_supply").ok()?;
    for e in dir.flatten() {
        let p = e.path();
        let is_batt = std::fs::read_to_string(p.join("type"))
            .map(|t| t.trim() == "Battery")
            .unwrap_or(false);
        if !is_batt {
            continue;
        }
        let cap: u32 = std::fs::read_to_string(p.join("capacity"))
            .ok()
            .and_then(|s| s.trim().parse().ok())?;
        let charging = std::fs::read_to_string(p.join("status"))
            .map(|s| {
                let s = s.trim();
                s == "Charging" || s == "Full"
            })
            .unwrap_or(false);
        return Some((cap.min(100), charging));
    }
    None
}

/// Date "wkd dd mon" (lowercase Spanish, e.g. "mar 21 jul") for the top bar's
/// date pill — the complement of the bottom bar's HH:MM clock. Spanish to
/// match the rest of the UI ("aplicaciones"); FONT_9X15 is ISO-8859-1 so the
/// accented "mié"/"sáb" render correctly.
pub fn date_dm() -> String {
    const WD: [&str; 7] = ["dom", "lun", "mar", "mié", "jue", "vie", "sáb"];
    const MO: [&str; 12] = [
        "ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sep", "oct", "nov", "dic",
    ];
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = core::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return "-- --".into();
        }
        let wd = WD.get(tm.tm_wday as usize).copied().unwrap_or("");
        let mo = MO.get(tm.tm_mon as usize).copied().unwrap_or("");
        format!("{} {:02} {}", wd, tm.tm_mday, mo)
    }
}

// ── Calendar support (the clock-click popup) ─────────────────────────────────

/// Full Spanish month names for the calendar header.
pub const MONTH_FULL: [&str; 12] = [
    "enero", "febrero", "marzo", "abril", "mayo", "junio", "julio", "agosto",
    "septiembre", "octubre", "noviembre", "diciembre",
];

/// Today's local (year, month 0-11, day 1-31), or None if localtime fails.
pub fn today() -> Option<(i32, u32, u32)> {
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = core::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return None;
        }
        Some((tm.tm_year + 1900, tm.tm_mon as u32, tm.tm_mday as u32))
    }
}

/// Number of days in the given month (0-based), Gregorian leap rules.
pub fn days_in_month(y: i32, m0: u32) -> u32 {
    const D: [u32; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if m0 == 1 && (y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)) {
        29
    } else {
        D[m0 as usize % 12]
    }
}

/// Weekday (0 = Monday .. 6 = Sunday) of the 1st of the given month, via the
/// standard days-from-civil algorithm (1970-01-01, day 0, was a Thursday).
pub fn first_weekday_mon0(y: i32, m0: u32) -> u32 {
    let (y, m, d) = (y as i64, m0 as i64 + 1, 1i64);
    let yy = if m <= 2 { y - 1 } else { y };
    let era = if yy >= 0 { yy } else { yy - 399 } / 400;
    let yoe = yy - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    ((days + 3).rem_euclid(7)) as u32 // day 0 Thursday → Monday-first index 3
}

/// System audio volume percent (0..100). Returns None when no supported
/// audio subsystem is present; the volume module is hidden in that case.
/// Setting the volume is handled by spawning amixer/wpctl from the popup.
///
/// Forks a helper once (startup / after the user changes the slider) — never
/// call this from the 1 Hz tick.
pub fn volume() -> Option<u32> {
    if let Some(v) = volume_wpctl() {
        return Some(v);
    }
    volume_amixer()
}

fn volume_wpctl() -> Option<u32> {
    let out = std::process::Command::new("wpctl")
        .args(["get-volume", "@DEFAULT_AUDIO_SINK@"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    // "Volume: 0.50" or "Volume: 0.50 [MUTED]"
    let frac: f64 = s
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()?;
    Some((frac * 100.0).round().clamp(0.0, 100.0) as u32)
}

fn volume_amixer() -> Option<u32> {
    let out = std::process::Command::new("amixer")
        .args(["get", "Master"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    // Prefer the first "[NN%]" token.
    for part in s.split('[') {
        if let Some(rest) = part.strip_suffix("%]") {
            if let Ok(v) = rest.parse::<u32>() {
                return Some(v.min(100));
            }
        }
        // amixer sometimes prints "NN%] [on]" without a second bracket close
        // in the same split — also accept "NN%" prefix.
        if let Some((num, _)) = part.split_once('%') {
            if let Ok(v) = num.parse::<u32>() {
                return Some(v.min(100));
            }
        }
    }
    None
}

