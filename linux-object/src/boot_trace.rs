//! Boot-time file-access recorder — "reverse-engineering" the desktop startup.
//!
//! The desktop compositor (labwc) reaches its first frame only after the
//! dynamic linker has mapped ~30-40 shared objects, xkbcommon has parsed the
//! keymap out of dozens of tiny `/usr/share/X11/xkb` files, and fontconfig /
//! theme / config files have been read — all of it, on a cold boot, a storm of
//! scattered 4 KiB demand-page reads over files that were never in the block
//! cache. To attack that we first have to SEE it.
//!
//! This module records, for exactly one target process (matched by `comm`,
//! selected at boot via `BOOTTRACE=<comm>` on the kernel command line), every
//! file it opens, with a monotonic timestamp and the open's result. The result
//! is surfaced as plain text at `/proc/bootprofile`: a timeline (which reveals
//! the phases — libraries first, then xkb, then fonts — and the stalls between
//! them) followed by a deduplicated, access-ordered *preload list*.
//!
//! That preload list is the point: it is the exact set of files, in the exact
//! order, that a later prefetch stage can stream into the block cache during
//! the idle window Eclipse already spends waiting for seatd and for
//! `/dev/input` to settle — so that when labwc actually starts, its working set
//! is already warm. The recorder here is deliberately the *first half* of that
//! system, not throwaway instrumentation.
//!
//! Cost when disabled (the default): one relaxed atomic load per `openat`.
//! Nothing else runs, nothing is allocated, until `BOOTTRACE=` arms it.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt::Write;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use lock::Mutex;

/// Master gate. Flipped true exactly once, by [`set_target`], when the boot
/// code sees `BOOTTRACE=<comm>`. Every hook checks this first and returns on a
/// single relaxed load when it is false.
static ENABLED: AtomicBool = AtomicBool::new(false);

/// The `comm` we are hunting for, lower-cased once at boot. Read-only after
/// [`set_target`]; guarded by a mutex only because it is a `String`.
static TARGET: Mutex<Option<String>> = Mutex::new(None);

/// KoID of the process we locked onto (its `comm` first matched [`TARGET`]).
/// `0` until armed. Once non-zero, only that pid's opens are recorded, so the
/// trace is a clean single-process capture even though every process's opens
/// flow through [`record_open`].
static ARMED_PID: AtomicU64 = AtomicU64::new(0);

/// Nanosecond timestamp (uptime) at which we armed — the `+0.000` of the
/// timeline.
static T0_NS: AtomicU64 = AtomicU64::new(0);

/// Uptime nanoseconds at arm time, kept for the header only.
static T0_UPTIME_NS: AtomicU64 = AtomicU64::new(0);

/// One recorded event, timestamped at [`dt_us`] microseconds since [`T0_NS`].
enum Rec {
    /// An `open`/`openat`: `result >= 0` is the fd, `< 0` is `-errno`.
    Open { dt_us: u64, result: i32, path: String },
    /// A syscall that took longer than [`SLOW_SYSCALL_US`] — the raw material
    /// for the gaps in the open timeline where NO file is touched (a blocking
    /// wait, or one very long call). `dur_us` is how long the call itself took.
    Slow { dt_us: u64, num: u32, dur_us: u64 },
}

impl Rec {
    fn dt_us(&self) -> u64 {
        match self {
            Rec::Open { dt_us, .. } | Rec::Slow { dt_us, .. } => *dt_us,
        }
    }
}

/// Threshold above which a syscall is worth recording on its own. Chosen so the
/// multi-second desktop stalls (icon-theme init, first render) are captured
/// without flooding the ring with ordinary fast calls.
const SLOW_SYSCALL_US: u64 = 300_000;

/// Bounded so a long-lived compositor cannot grow the log without end: startup
/// is what we care about, and 8192 opens comfortably covers a full desktop
/// bring-up. Once full, further opens are counted (see [`DROPPED`]) but not
/// stored.
const MAX_RECS: usize = 8192;
static RECS: Mutex<Vec<Rec>> = Mutex::new(Vec::new());
static DROPPED: AtomicU64 = AtomicU64::new(0);

/// Arm the recorder for processes whose `comm` equals `target`. Called once
/// from boot when `BOOTTRACE=<comm>` is present. Idempotent.
pub fn set_target(target: &str) {
    let mut t = TARGET.lock();
    if t.is_none() {
        *t = Some(target.to_string());
        ENABLED.store(true, Ordering::Release);
    }
}

/// True once [`set_target`] has armed the recorder. One relaxed load; this is
/// the whole cost on the `openat` fast path when tracing is off.
#[inline]
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

fn now_ns() -> u64 {
    kernel_hal::timer::timer_now().as_nanos() as u64
}

/// Record one `openat`/`open`. `comm` is computed lazily — the closure only
/// runs while we are still hunting for the target (before arming), never on the
/// steady-state path once locked onto a pid.
///
/// `result` is the syscall's return: `>= 0` an fd, `< 0` a `-errno`. Errors are
/// recorded on purpose: the dynamic linker's ENOENT probes across its library
/// search path are exactly what reveal how it hunts for each `.so`.
pub fn record_open(pid: u64, comm: impl FnOnce() -> String, path: &str, result: i32) {
    if !enabled() {
        return;
    }
    let armed = ARMED_PID.load(Ordering::Relaxed);
    if armed == 0 {
        // Still hunting: pay for the comm string and the compare.
        let target_matches = {
            let t = TARGET.lock();
            match t.as_deref() {
                Some(target) => comm() == target,
                None => false,
            }
        };
        if !target_matches {
            return;
        }
        // First matching open wins the arm. A concurrent opener of the same
        // comm (there is only one labwc, but be correct) either loses the CAS
        // and, unless it is the same pid, is dropped.
        match ARMED_PID.compare_exchange(0, pid, Ordering::AcqRel, Ordering::Relaxed) {
            Ok(_) => {
                let now = now_ns();
                T0_NS.store(now, Ordering::Relaxed);
                T0_UPTIME_NS.store(now, Ordering::Relaxed);
            }
            Err(winner) if winner != pid => return,
            Err(_) => {}
        }
    } else if armed != pid {
        return;
    }
    let dt_us = elapsed_us();
    push(Rec::Open {
        dt_us,
        result,
        path: path.to_string(),
    });
}

/// Record a syscall that took `dur_ns`, but only if it crossed
/// [`SLOW_SYSCALL_US`] and belongs to the armed process. Called from the
/// syscall dispatcher's per-call timing. Cheap and lazy: returns on the
/// enabled/armed/threshold checks before touching the lock.
pub fn record_syscall(pid: u64, num: u32, dur_ns: u64) {
    if !enabled() || dur_ns / 1000 < SLOW_SYSCALL_US {
        return;
    }
    if ARMED_PID.load(Ordering::Relaxed) != pid {
        return;
    }
    let dt_us = elapsed_us();
    push(Rec::Slow {
        dt_us,
        num,
        dur_us: dur_ns / 1000,
    });
}

fn elapsed_us() -> u64 {
    now_ns().saturating_sub(T0_NS.load(Ordering::Relaxed)) / 1000
}

fn push(rec: Rec) {
    let mut recs = RECS.lock();
    if recs.len() >= MAX_RECS {
        DROPPED.fetch_add(1, Ordering::Relaxed);
        return;
    }
    recs.push(rec);
}

/// The deduplicated, access-ordered list of paths that were opened
/// SUCCESSFULLY by the traced process — the prefetch list for the replay stage.
/// Empty until a trace has been captured.
pub fn preload_list() -> Vec<String> {
    let recs = RECS.lock();
    let mut out: Vec<String> = Vec::new();
    for r in recs.iter() {
        if let Rec::Open { result, path, .. } = r {
            if *result >= 0 && !out.iter().any(|p| p == path) {
                out.push(path.clone());
            }
        }
    }
    out
}

/// Render `/proc/bootprofile`.
pub fn render() -> String {
    if !enabled() {
        return "boot-trace: disabled.\n\
                Enable by adding BOOTTRACE=<comm> to the kernel command line \
                (e.g. BOOTTRACE=labwc), reboot, then read /proc/bootprofile \
                after the desktop is up.\n"
            .to_string();
    }
    let target = TARGET.lock().clone().unwrap_or_default();
    let armed = ARMED_PID.load(Ordering::Relaxed);
    let mut out = String::new();

    if armed == 0 {
        let _ = writeln!(
            out,
            "eclipse boot-trace — target '{}': ARMED, no matching process has opened a file yet.",
            target
        );
        return out;
    }

    let recs = RECS.lock();
    let dropped = DROPPED.load(Ordering::Relaxed);
    let t0_uptime_ms = T0_UPTIME_NS.load(Ordering::Relaxed) / 1_000_000;
    let (mut ok, mut miss, mut slow) = (0u32, 0u32, 0u32);
    for r in recs.iter() {
        match r {
            Rec::Open { result, .. } if *result >= 0 => ok += 1,
            Rec::Open { .. } => miss += 1,
            Rec::Slow { .. } => slow += 1,
        }
    }
    let window_ms = recs.last().map(|r| r.dt_us() as f64 / 1000.0).unwrap_or(0.0);

    let _ = writeln!(
        out,
        "eclipse boot-trace — target '{}', pid {}",
        target, armed
    );
    let _ = writeln!(
        out,
        "armed at {}.{:03}s uptime; window {:.1}ms; {} opens ({} ok, {} miss), {} slow syscalls (>{}ms){}",
        t0_uptime_ms / 1000,
        t0_uptime_ms % 1000,
        window_ms,
        ok + miss,
        ok,
        miss,
        slow,
        SLOW_SYSCALL_US / 1000,
        if dropped > 0 {
            alloc::format!("; {dropped} DROPPED (log full at {MAX_RECS})")
        } else {
            String::new()
        }
    );
    let _ = writeln!(
        out,
        "a '*' marks a gap > 2ms since the previous event; SLOW lines are single \
         syscalls that themselves took >{}ms (the blocking waits inside the gaps \
         with no file activity — first render, icon-theme init, GPU waits):",
        SLOW_SYSCALL_US / 1000
    );
    let _ = writeln!(out, "\n   time(ms)   d(ms)  res  path / syscall");

    let mut prev_us = 0u64;
    for r in recs.iter() {
        let gap_us = r.dt_us().saturating_sub(prev_us);
        let stall = if gap_us > 2000 { '*' } else { ' ' };
        match r {
            Rec::Open { dt_us, result, path } => {
                let res = if *result >= 0 {
                    "ok  ".to_string()
                } else {
                    alloc::format!("e{:<3}", -*result)
                };
                let _ = writeln!(
                    out,
                    "  {:>8.3} {:>7.2} {} {} {}",
                    *dt_us as f64 / 1000.0,
                    gap_us as f64 / 1000.0,
                    stall,
                    res,
                    path
                );
            }
            Rec::Slow { dt_us, num, dur_us } => {
                let _ = writeln!(
                    out,
                    "  {:>8.3} {:>7.2} {} SLOW {} took {:.1}ms",
                    *dt_us as f64 / 1000.0,
                    gap_us as f64 / 1000.0,
                    stall,
                    crate::perf::name_of(*num),
                    *dur_us as f64 / 1000.0,
                );
            }
        }
        prev_us = r.dt_us();
    }

    // The prefetch list: ok-only, deduped, in first-access order.
    let mut seen: Vec<&str> = Vec::new();
    for r in recs.iter() {
        if let Rec::Open { result, path, .. } = r {
            if *result >= 0 && !seen.contains(&path.as_str()) {
                seen.push(path.as_str());
            }
        }
    }
    let _ = writeln!(
        out,
        "\n--- preload list (ok opens, deduped, in access order): {} files ---",
        seen.len()
    );
    for p in &seen {
        let _ = writeln!(out, "{p}");
    }
    out
}
