//! Behavioural anomaly detection (the IDS half of hunter).
//!
//! Signals produced on the syscall hot path, *after* the policy check (so they
//! only observe calls that were allowed to run):
//!
//! 1. **Sensitive-syscall watch** — constant-time classification against a
//!    per-architecture table of security-relevant operations (module loading,
//!    `ptrace`, `bpf`, credential / namespace changes, `kexec`, …). Matches are
//!    logged; with the optional privileged-deny latch they can also be blocked.
//! 2. **Rate anomalies** — per-process sliding-window counters that flag
//!    syscall **floods** and **fork bombs**, plus a **system-wide** fork-rate
//!    signal so a *distributed* fork bomb (each child forks once) still trips.
//! 3. **Adaptive baseline** — an EWMA of each process's syscalls-per-window,
//!    flagging a window that spikes far above *that process's own history*.
//!    This catches a process-relative flood (e.g. a quiet 200/s task jumping
//!    to 8k/s) that stays under the absolute flood threshold, while the
//!    absolute threshold still backstops a process whose baseline is high.
//!
//! Hardening: per-arch syscall numbers (P14, finding SYS-3/IDS-3); a
//! count-based window backstop so a frozen clock cannot silence detection
//! (P12); bounded per-process state with LRU eviction (P5); throttled WATCH
//! events so an attacker cannot use them as cheap log-ring filler (P11); and an
//! optional `Enforce` mode that actually denies confirmed anomalies (P15).

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::format;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use lock::Mutex;

use crate::clock;
use crate::event_log::{record, Severity};
use crate::policy::{self, Mode};

/// Master switch for the (locked) rate heuristics. The sensitive-syscall watch
/// is always cheap and stays on regardless.
///
/// Default OFF: with it on, EVERY syscall of every process pays a clock read
/// plus an IRQ-off shard-mutex + BTreeMap update, serializing all threads of a
/// process at syscall entry — a measurable tax on exactly the workloads a
/// desktop session hammers (Wayland/pipe round-trips are several syscalls
/// each). The constant-time sensitive-syscall WATCH below stays on either way;
/// only the flood/fork-bomb rate counters are opt-in (`HUNTERANOMALY=1`).
static ANOMALY_ENABLED: AtomicBool = AtomicBool::new(false);
/// Opt-in latch: when set and the anomaly domain is `Enforce`, syscalls in the
/// privileged class are denied outright (default off — never breaks boot).
static PRIVILEGED_DENY: AtomicBool = AtomicBool::new(false);

/// Sliding-window length for the rate heuristics.
const WINDOW_NS: u64 = 1_000_000_000; // 1 second
/// Per-process syscalls/window above which we suspect a denial-of-service flood.
const FLOOD_THRESHOLD: u32 = 50_000;
/// Adaptive flood: a window whose syscall count exceeds this multiple of the
/// process's own EWMA baseline is flagged, catching process-relative spikes
/// that stay below the absolute `FLOOD_THRESHOLD`.
const ADAPTIVE_MULT: u32 = 20;
/// Floor below which the adaptive detector stays silent, so low-volume or
/// merely bursty-but-benign processes never trip it.
const ADAPTIVE_MIN_COUNT: u32 = 2_000;
/// Clean (un-flagged) windows of history required before the adaptive baseline
/// is trusted enough to fire, so a process's first windows just train it.
const ADAPTIVE_MIN_WINDOWS: u16 = 4;
/// EWMA smoothing shift for the adaptive baseline: `ewma += (sample - ewma) >> N`
/// (alpha = 1/8). Slow enough that a ramp-up cannot quietly poison the baseline.
const EWMA_SHIFT: u32 = 3;
/// Per-process clone/fork calls/window above which we suspect a fork bomb.
const FORKBOMB_THRESHOLD: u32 = 200;
/// System-wide clone/fork calls/window flagged as a distributed fork bomb.
const SYS_FORKBOMB_THRESHOLD: u64 = 2_000;
/// Count backstop: roll the window after this many syscalls even if the clock
/// has not advanced, so a frozen/lying clock cannot disable detection (P12).
const WINDOW_EVENTS_BACKSTOP: u32 = 1_000_000;
/// WATCH events emitted per process per window before further ones are
/// suppressed (still counted), bounding self-inflicted log pressure (P11).
const WATCH_BUDGET: u32 = 16;
/// Maximum processes tracked before the least-recently-active is evicted (P5).
const MAX_TRACKED_PIDS: usize = 4096;

/// Per-architecture syscall numbers. Absent operations use `u32::MAX` as a
/// sentinel that never matches a real syscall number.
mod nr {
    pub const ABSENT: u32 = u32::MAX;

    #[cfg(target_arch = "x86_64")]
    mod imp {
        pub const PTRACE: u32 = 101;
        pub const SETUID: u32 = 105;
        pub const SETGID: u32 = 106;
        pub const SETREUID: u32 = 113;
        pub const SETRESUID: u32 = 117;
        pub const PIVOT_ROOT: u32 = 155;
        pub const CHROOT: u32 = 161;
        pub const MOUNT: u32 = 165;
        pub const REBOOT: u32 = 169;
        pub const INIT_MODULE: u32 = 175;
        pub const DELETE_MODULE: u32 = 176;
        pub const KEXEC_LOAD: u32 = 246;
        pub const KEYCTL: u32 = 250;
        pub const UNSHARE: u32 = 272;
        pub const SETNS: u32 = 308;
        pub const PROCESS_VM_WRITEV: u32 = 311;
        pub const FINIT_MODULE: u32 = 313;
        pub const KEXEC_FILE_LOAD: u32 = 320;
        pub const BPF: u32 = 321;
        pub const CLONE: u32 = 56;
        pub const FORK: u32 = 57;
        pub const VFORK: u32 = 58;
    }

    // aarch64 and riscv64 use the asm-generic syscall table.
    #[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
    mod imp {
        pub const PTRACE: u32 = 117;
        pub const SETUID: u32 = 146;
        pub const SETGID: u32 = 144;
        pub const SETREUID: u32 = 145;
        pub const SETRESUID: u32 = 147;
        pub const PIVOT_ROOT: u32 = 41;
        pub const CHROOT: u32 = 51;
        pub const MOUNT: u32 = 40;
        pub const REBOOT: u32 = 142;
        pub const INIT_MODULE: u32 = 105;
        pub const DELETE_MODULE: u32 = 106;
        pub const KEXEC_LOAD: u32 = 104;
        pub const KEYCTL: u32 = 219;
        pub const UNSHARE: u32 = 97;
        pub const SETNS: u32 = 268;
        pub const PROCESS_VM_WRITEV: u32 = 271;
        pub const FINIT_MODULE: u32 = 273;
        pub const KEXEC_FILE_LOAD: u32 = 294;
        pub const BPF: u32 = 280;
        pub const CLONE: u32 = 220;
        pub const FORK: u32 = super::ABSENT; // no fork/vfork in asm-generic
        pub const VFORK: u32 = super::ABSENT;
    }

    #[cfg(not(any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "riscv64"
    )))]
    mod imp {
        pub const PTRACE: u32 = super::ABSENT;
        pub const SETUID: u32 = super::ABSENT;
        pub const SETGID: u32 = super::ABSENT;
        pub const SETREUID: u32 = super::ABSENT;
        pub const SETRESUID: u32 = super::ABSENT;
        pub const PIVOT_ROOT: u32 = super::ABSENT;
        pub const CHROOT: u32 = super::ABSENT;
        pub const MOUNT: u32 = super::ABSENT;
        pub const REBOOT: u32 = super::ABSENT;
        pub const INIT_MODULE: u32 = super::ABSENT;
        pub const DELETE_MODULE: u32 = super::ABSENT;
        pub const KEXEC_LOAD: u32 = super::ABSENT;
        pub const KEYCTL: u32 = super::ABSENT;
        pub const UNSHARE: u32 = super::ABSENT;
        pub const SETNS: u32 = super::ABSENT;
        pub const PROCESS_VM_WRITEV: u32 = super::ABSENT;
        pub const FINIT_MODULE: u32 = super::ABSENT;
        pub const KEXEC_FILE_LOAD: u32 = super::ABSENT;
        pub const BPF: u32 = super::ABSENT;
        pub const CLONE: u32 = super::ABSENT;
        pub const FORK: u32 = super::ABSENT;
        pub const VFORK: u32 = super::ABSENT;
    }

    pub use imp::*;
}

/// `true` if `num` equals a present (non-sentinel) syscall constant.
#[inline]
fn is(num: u32, c: u32) -> bool {
    c != nr::ABSENT && num == c
}

/// Returns `(category, severity, name)` for security-sensitive syscalls.
fn classify(num: u32) -> Option<(&'static str, Severity, &'static str)> {
    if is(num, nr::INIT_MODULE) || is(num, nr::FINIT_MODULE) {
        return Some(("MODULE", Severity::Warning, "kernel module load"));
    }
    if is(num, nr::DELETE_MODULE) {
        return Some(("MODULE", Severity::Warning, "kernel module unload"));
    }
    if is(num, nr::KEXEC_LOAD) || is(num, nr::KEXEC_FILE_LOAD) {
        return Some(("MODULE", Severity::Warning, "kexec load"));
    }
    if is(num, nr::BPF) {
        return Some(("PRIVILEGE", Severity::Notice, "bpf"));
    }
    if is(num, nr::PTRACE) {
        return Some(("PRIVILEGE", Severity::Notice, "ptrace"));
    }
    if is(num, nr::SETUID) || is(num, nr::SETGID) || is(num, nr::SETREUID) || is(num, nr::SETRESUID)
    {
        return Some(("PRIVILEGE", Severity::Notice, "credential change"));
    }
    if is(num, nr::MOUNT) || is(num, nr::PIVOT_ROOT) || is(num, nr::CHROOT) {
        return Some(("PRIVILEGE", Severity::Notice, "fs namespace"));
    }
    if is(num, nr::UNSHARE) || is(num, nr::SETNS) {
        return Some(("PRIVILEGE", Severity::Notice, "namespace change"));
    }
    if is(num, nr::PROCESS_VM_WRITEV) {
        return Some(("PRIVILEGE", Severity::Notice, "cross-process write"));
    }
    if is(num, nr::KEYCTL) {
        return Some(("PRIVILEGE", Severity::Notice, "keyring"));
    }
    if is(num, nr::REBOOT) {
        return Some(("PRIVILEGE", Severity::Notice, "reboot"));
    }
    None
}

/// Whether `num` is in the privileged class that the deny latch may block.
fn is_privileged(num: u32) -> bool {
    is(num, nr::INIT_MODULE)
        || is(num, nr::FINIT_MODULE)
        || is(num, nr::DELETE_MODULE)
        || is(num, nr::KEXEC_LOAD)
        || is(num, nr::KEXEC_FILE_LOAD)
        || is(num, nr::BPF)
        || is(num, nr::PTRACE)
        || is(num, nr::MOUNT)
        || is(num, nr::PIVOT_ROOT)
        || is(num, nr::SETNS)
}

#[inline]
fn is_fork(num: u32) -> bool {
    is(num, nr::CLONE) || is(num, nr::FORK) || is(num, nr::VFORK)
}

/// Per-process sliding-window state for the rate heuristics.
struct ProcStat {
    window_start: u64,
    syscall_count: u32,
    fork_count: u32,
    watch_count: u32,
    flood_alerted: bool,
    fork_alerted: bool,
    /// Adaptive (process-relative) flood already reported this window.
    adaptive_alerted: bool,
    /// EWMA of syscalls-per-window — the process's learned baseline.
    ewma_syscalls: u32,
    /// Count of clean windows folded into `ewma_syscalls` (warm-up gate).
    windows_observed: u16,
}

impl ProcStat {
    fn new(now: u64) -> Self {
        Self {
            window_start: now,
            syscall_count: 0,
            fork_count: 0,
            watch_count: 0,
            flood_alerted: false,
            fork_alerted: false,
            adaptive_alerted: false,
            ewma_syscalls: 0,
            windows_observed: 0,
        }
    }
    /// Resets the window if the clock advanced past it OR the event backstop
    /// tripped (the latter keeps windows progressing under a frozen clock).
    fn roll(&mut self, now: u64) {
        let elapsed = now.saturating_sub(self.window_start) >= WINDOW_NS;
        let backstop = self.syscall_count >= WINDOW_EVENTS_BACKSTOP;
        if !(elapsed || backstop) {
            return;
        }
        // Fold a *completed time* window into the adaptive baseline — but never
        // a backstop window (an in-progress flood, not a full second) and never
        // a window we already flagged, so an attacker cannot train the baseline
        // upward to silence future detection.
        if elapsed && !self.flood_alerted && !self.adaptive_alerted {
            let sample = self.syscall_count;
            let ewma = self.ewma_syscalls;
            self.ewma_syscalls = if sample >= ewma {
                ewma + ((sample - ewma) >> EWMA_SHIFT)
            } else {
                ewma - ((ewma - sample) >> EWMA_SHIFT)
            };
            self.windows_observed = self.windows_observed.saturating_add(1);
        }
        self.window_start = now;
        self.syscall_count = 0;
        self.fork_count = 0;
        self.watch_count = 0;
        self.flood_alerted = false;
        self.fork_alerted = false;
        self.adaptive_alerted = false;
    }
    /// Whether the current window is a flood *relative to this process's own*
    /// learned baseline. Pure (no clock, no logging) so it is unit-testable.
    fn adaptive_flood(&self) -> bool {
        self.windows_observed >= ADAPTIVE_MIN_WINDOWS
            && self.syscall_count > ADAPTIVE_MIN_COUNT
            && (self.syscall_count as u64) > (ADAPTIVE_MULT as u64) * (self.ewma_syscalls as u64)
    }
}

/// Number of independently-locked shards for the per-process stats. The map
/// is written on the syscall hot path from every CPU; a single global lock
/// serialized all of them (and disabled IRQs while doing so). Sharding by pid
/// keeps unrelated processes on unrelated locks/cachelines — same-pid calls
/// still serialize, which the per-pid counters require anyway.
const STAT_SHARDS: usize = 16;

/// One shard, padded to its own cacheline so neighbouring shard locks do not
/// false-share.
#[repr(align(64))]
struct StatShard(Mutex<BTreeMap<u64, ProcStat>>);

lazy_static::lazy_static! {
    static ref PROC_STATS: [StatShard; STAT_SHARDS] = {
        // `Mutex::new` is const but arrays of non-Copy need explicit build.
        core::array::from_fn(|_| StatShard(Mutex::new(BTreeMap::new())))
    };
}

#[inline]
fn stats_shard(pid: u64) -> &'static Mutex<BTreeMap<u64, ProcStat>> {
    &PROC_STATS[(pid as usize) % STAT_SHARDS].0
}

/// System-wide fork accounting for distributed fork-bomb detection.
static SYS_FORK_WINDOW_START: AtomicU64 = AtomicU64::new(0);
static SYS_FORK_COUNT: AtomicU64 = AtomicU64::new(0);
static SYS_FORK_ALERTED: AtomicBool = AtomicBool::new(false);

/// Enables or disables the per-process rate heuristics.
pub fn set_anomaly_detection(enabled: bool) {
    ANOMALY_ENABLED.store(enabled, Ordering::Relaxed);
}

/// Enables/disables the privileged-syscall deny latch (only bites when the
/// anomaly domain is also `Enforce`). Default off; opt-in only.
pub fn set_privileged_deny(enabled: bool) {
    PRIVILEGED_DENY.store(enabled, Ordering::Relaxed);
}

/// Inspects one syscall for anomalies. Returns `true` to allow the call, or
/// `false` to deny it (only ever happens when the anomaly domain is `Enforce`).
/// Called on the syscall hot path *after* the policy check.
pub fn on_syscall(pid: u64, num: u32) -> bool {
    // (1) Constant-time sensitive-syscall classification (no lock, no clock).
    let classified = classify(num);
    let anomaly = ANOMALY_ENABLED.load(Ordering::Relaxed);

    // Hot-path early out: an ordinary syscall with the rate heuristics off
    // touches no shared state at all.
    if classified.is_none() && !anomaly {
        return true;
    }

    let enforce = policy::anomaly_mode() == Mode::Enforce;

    if let Some((category, _severity, name)) = classified {
        // Privileged-deny latch: block module/ptrace/bpf/... under Enforce.
        if enforce && PRIVILEGED_DENY.load(Ordering::Relaxed) && is_privileged(num) {
            record(
                pid,
                Severity::Critical,
                category,
                "BLOCKED",
                format!("blocked privileged syscall #{} ({})", num, name),
            );
            return false;
        }
    }

    let now = clock::now_ns();
    let forking = anomaly && is_fork(num);

    // Single pass under the pid's shard lock: WATCH throttling and the rate
    // counters share one roll of the window (the original code locked and
    // rolled twice for sensitive syscalls).
    let mut emit_watch = false;
    let mut alert_flood = false;
    let mut alert_fork = false;
    let mut alert_adaptive = false;
    // Per-process window count and baseline captured under the lock for the
    // adaptive anomaly message.
    let mut spike_count = 0u32;
    let mut baseline = 0u32;
    {
        let mut stats = stats_shard(pid).lock();
        evict_if_needed(&mut stats, pid);
        let st = stats.entry(pid).or_insert_with(|| ProcStat::new(now));
        st.roll(now);
        if classified.is_some() {
            // Throttled WATCH so an attacker cannot use these as log-ring filler.
            st.watch_count = st.watch_count.saturating_add(1);
            emit_watch = st.watch_count <= WATCH_BUDGET;
        }
        if anomaly {
            st.syscall_count = st.syscall_count.saturating_add(1);
            if forking {
                st.fork_count = st.fork_count.saturating_add(1);
            }
            if !st.flood_alerted && st.syscall_count > FLOOD_THRESHOLD {
                st.flood_alerted = true;
                alert_flood = true;
            }
            // Adaptive flood: suppressed once the absolute flood already fired
            // this window, since that path reports the same burst more precisely.
            if !st.flood_alerted && !st.adaptive_alerted && st.adaptive_flood() {
                st.adaptive_alerted = true;
                alert_adaptive = true;
                spike_count = st.syscall_count;
                baseline = st.ewma_syscalls;
            }
            if !st.fork_alerted && st.fork_count > FORKBOMB_THRESHOLD {
                st.fork_alerted = true;
                alert_fork = true;
            }
        }
    }
    if let (true, Some((category, severity, name))) = (emit_watch, classified) {
        record(
            pid,
            severity,
            category,
            "WATCH",
            format!("sensitive syscall #{} ({})", num, name),
        );
    }
    if !anomaly {
        return true;
    }

    // System-wide fork-rate window (lock-free), for distributed fork bombs.
    let mut alert_sys_fork = false;
    if forking {
        let prev = SYS_FORK_WINDOW_START.load(Ordering::Relaxed);
        if now.saturating_sub(prev) >= WINDOW_NS {
            SYS_FORK_WINDOW_START.store(now, Ordering::Relaxed);
            SYS_FORK_COUNT.store(0, Ordering::Relaxed);
            SYS_FORK_ALERTED.store(false, Ordering::Relaxed);
        }
        let total = SYS_FORK_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        if total > SYS_FORKBOMB_THRESHOLD && !SYS_FORK_ALERTED.swap(true, Ordering::Relaxed) {
            alert_sys_fork = true;
        }
    }

    let mut deny = false;
    if alert_flood {
        log_anomaly(
            pid,
            enforce,
            format!(
                "syscall flood: >{} syscalls within {}ms",
                FLOOD_THRESHOLD,
                WINDOW_NS / 1_000_000
            ),
        );
        deny |= enforce;
    }
    if alert_adaptive {
        log_anomaly(
            pid,
            enforce,
            format!(
            "adaptive syscall spike: {} within {}ms is >{}x the per-process baseline (~{}/window)",
            spike_count,
            WINDOW_NS / 1_000_000,
            ADAPTIVE_MULT,
            baseline
        ),
        );
        deny |= enforce;
    }
    if alert_fork {
        log_anomaly(
            pid,
            enforce,
            format!(
                "possible fork bomb: >{} clone/fork within {}ms",
                FORKBOMB_THRESHOLD,
                WINDOW_NS / 1_000_000
            ),
        );
        deny |= enforce;
    }
    if alert_sys_fork {
        log_anomaly(
            pid,
            enforce,
            format!(
                "system-wide fork storm: >{} clone/fork within {}ms",
                SYS_FORKBOMB_THRESHOLD,
                WINDOW_NS / 1_000_000
            ),
        );
        deny |= enforce;
    }

    !deny
}

fn log_anomaly(pid: u64, enforce: bool, msg: alloc::string::String) {
    if enforce {
        record(pid, Severity::Critical, "ANOMALY", "BLOCKED", msg);
    } else {
        record(pid, Severity::Warning, "ANOMALY", "WARNING", msg);
    }
}

/// Resets a process's anomaly window across `execve` so a benign-then-malicious
/// image transition cannot launder accumulated counters (P4).
pub fn on_exec(pid: u64) {
    let now = clock::now_ns();
    stats_shard(pid).lock().insert(pid, ProcStat::new(now));
}

/// Drops per-process heuristic state when a process exits.
pub fn forget(pid: u64) {
    stats_shard(pid).lock().remove(&pid);
}

/// Evicts the least-recently-active process if inserting `pid` would exceed the
/// cap (and `pid` is not already tracked), bounding memory under spawn floods.
fn evict_if_needed(map: &mut BTreeMap<u64, ProcStat>, pid: u64) {
    // `map` is one shard; bound each shard to its slice of the global cap so
    // total tracked state stays at MAX_TRACKED_PIDS across all shards.
    if map.len() < MAX_TRACKED_PIDS / STAT_SHARDS || map.contains_key(&pid) {
        return;
    }
    if let Some((&victim, _)) = map.iter().min_by_key(|(_, st)| st.window_start) {
        map.remove(&victim);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Drives `ProcStat` directly with synthetic timestamps so the adaptive
    // baseline is exercised without the global (sealed) clock.
    #[test]
    fn adaptive_baseline_learns_then_flags_relative_spike() {
        let mut st = ProcStat::new(0);
        let mut t = 0u64;

        // Several calm windows of ~200 syscalls each train the baseline.
        for _ in 0..6 {
            st.syscall_count = 200;
            t += WINDOW_NS;
            st.roll(t);
        }
        assert!(st.windows_observed >= ADAPTIVE_MIN_WINDOWS);
        assert!(st.ewma_syscalls > 0 && st.ewma_syscalls < ADAPTIVE_MIN_COUNT);

        // A calm window — and a fork-heavy but low-volume one — do not trip.
        st.syscall_count = 200;
        assert!(!st.adaptive_flood());
        st.syscall_count = ADAPTIVE_MIN_COUNT; // exactly at the floor: still quiet
        assert!(!st.adaptive_flood());

        // A spike far above this process's own baseline (and above the floor)
        // trips, even though it is well below the absolute FLOOD_THRESHOLD.
        st.syscall_count = 8_000;
        assert!(st.syscall_count < FLOOD_THRESHOLD);
        assert!(st.adaptive_flood());
    }

    #[test]
    fn flagged_window_does_not_poison_baseline() {
        let mut st = ProcStat::new(0);
        let mut t = 0u64;
        for _ in 0..4 {
            st.syscall_count = 100;
            t += WINDOW_NS;
            st.roll(t);
        }
        let before = st.ewma_syscalls;

        // A window we already flagged must not be folded into the baseline,
        // otherwise an attacker could train it upward to silence detection.
        st.syscall_count = 1_000_000;
        st.adaptive_alerted = true;
        t += WINDOW_NS;
        st.roll(t);
        assert_eq!(st.ewma_syscalls, before);
    }

    #[test]
    fn cold_process_never_trips_adaptively() {
        // Before the warm-up gate, even a large count does not trip (no
        // trusted baseline yet); the absolute threshold still backstops it.
        let mut st = ProcStat::new(0);
        st.syscall_count = 40_000;
        assert!(st.windows_observed < ADAPTIVE_MIN_WINDOWS);
        assert!(!st.adaptive_flood());
    }
}
