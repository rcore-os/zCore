//! Eclipse's own lightweight kernel observability ("our own perf").
//!
//! Rather than emulating the Linux `perf` tool's ring-buffer ABI, this is a
//! homegrown, always-on accounting layer surfaced as plain text:
//!
//! - **`/proc/perf`** — system-wide syscall accounting (calls + time per
//!   syscall, busiest first).
//! - **`/proc/<pid>/perf`** — the same broken down for one process.
//!
//! The syscall dispatcher calls [`record`] once per syscall with the elapsed
//! time; everything here is lock-free atomics on the hot path. Report rendering
//! (rare) resolves syscall numbers to names through a resolver registered by
//! `linux-syscall` (which owns the `Sys` enum), so this crate needs no
//! arch-specific name table.

use crate::process::LinuxProcess;
use alloc::collections::BTreeMap;
use alloc::{boxed::Box, string::String, vec::Vec};
use core::fmt::Write;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering::Relaxed};
use lock::Mutex;

/// Size of the per-syscall tables. Must exceed the largest syscall number in
/// use (Eclipse's custom syscalls go up to ~601).
pub const PERF_NR: usize = 640;

/// System-wide call counts, indexed by syscall number.
static SYS_COUNT: [AtomicU64; PERF_NR] = [const { AtomicU64::new(0) }; PERF_NR];
/// System-wide cumulative time spent in each syscall, in nanoseconds.
static SYS_NS: [AtomicU64; PERF_NR] = [const { AtomicU64::new(0) }; PERF_NR];

/// Resolver from syscall number to a human name, registered by `linux-syscall`.
type NameResolver = fn(u32) -> Option<String>;
static NAME_RESOLVER: Mutex<Option<NameResolver>> = Mutex::new(None);

/// Register the syscall-name resolver. Called once by `linux-syscall`.
pub fn set_name_resolver(f: fn(u32) -> Option<String>) {
    *NAME_RESOLVER.lock() = Some(f);
}

/// Resolve a syscall number to a human name via the resolver registered by
/// `linux-syscall`, falling back to `sys_<n>`. Public so other observability
/// surfaces (e.g. `boot_trace`) can label syscalls too.
pub fn name_of(num: u32) -> String {
    if let Some(f) = *NAME_RESOLVER.lock() {
        if let Some(n) = f(num) {
            return n;
        }
    }
    alloc::format!("sys_{}", num)
}

/// Record one completed syscall: bump the global tables and the calling
/// process's own counters. `ns` is the wall-clock time the syscall took.
pub fn record(proc: &LinuxProcess, num: u32, ns: u64) {
    if (num as usize) < PERF_NR {
        SYS_COUNT[num as usize].fetch_add(1, Relaxed);
        SYS_NS[num as usize].fetch_add(ns, Relaxed);
    }
    proc.perf().record(num, ns);
}

/// Per-process syscall accounting, stored inline on [`LinuxProcess`] so it is
/// freed with the process.
pub struct ProcPerf {
    count: AtomicU64,
    ns: AtomicU64,
    /// Per-syscall call count.
    per: Box<[AtomicU32]>,
    /// Per-syscall cumulative time (ns), so `/proc/<pid>/perf` shows latency.
    per_ns: Box<[AtomicU64]>,
}

impl Default for ProcPerf {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcPerf {
    /// Create a zeroed per-process accounting table.
    pub fn new() -> Self {
        let mut per = Vec::with_capacity(PERF_NR);
        let mut per_ns = Vec::with_capacity(PERF_NR);
        for _ in 0..PERF_NR {
            per.push(AtomicU32::new(0));
            per_ns.push(AtomicU64::new(0));
        }
        ProcPerf {
            count: AtomicU64::new(0),
            ns: AtomicU64::new(0),
            per: per.into_boxed_slice(),
            per_ns: per_ns.into_boxed_slice(),
        }
    }

    fn record(&self, num: u32, ns: u64) {
        self.count.fetch_add(1, Relaxed);
        self.ns.fetch_add(ns, Relaxed);
        if (num as usize) < self.per.len() {
            self.per[num as usize].fetch_add(1, Relaxed);
            self.per_ns[num as usize].fetch_add(ns, Relaxed);
        }
    }

    /// `(total calls, total nanoseconds)`.
    pub fn totals(&self) -> (u64, u64) {
        (self.count.load(Relaxed), self.ns.load(Relaxed))
    }
}

fn fmt_table(out: &mut String, mut rows: Vec<(u32, u64, u64)>) {
    // Busiest syscall first.
    rows.sort_by_key(|r| core::cmp::Reverse(r.1));
    let _ = writeln!(
        out,
        "  {:<20} {:>12} {:>12} {:>10}",
        "SYSCALL", "CALLS", "TOTAL ms", "AVG us"
    );
    for (num, calls, ns) in rows {
        if calls == 0 {
            continue;
        }
        let total_ms = ns as f64 / 1_000_000.0;
        let avg_us = if calls > 0 {
            (ns as f64 / calls as f64) / 1000.0
        } else {
            0.0
        };
        let _ = writeln!(
            out,
            "  {:<20} {:>12} {:>12.3} {:>10.2}",
            name_of(num),
            calls,
            total_ms,
            avg_us
        );
    }
}

/// Render `/proc/perf`: system-wide syscall accounting.
pub fn global_report() -> String {
    let uptime = kernel_hal::timer::timer_now().as_secs_f64();
    let mut total_calls = 0u64;
    let mut rows: Vec<(u32, u64, u64)> = Vec::new();
    for i in 0..PERF_NR {
        let calls = SYS_COUNT[i].load(Relaxed);
        if calls != 0 {
            let ns = SYS_NS[i].load(Relaxed);
            total_calls += calls;
            rows.push((i as u32, calls, ns));
        }
    }
    let mut out = String::new();
    let _ = writeln!(out, "eclipse perf — system-wide syscall accounting");
    let _ = writeln!(out);
    let rate = if uptime > 0.0 {
        total_calls as f64 / uptime
    } else {
        0.0
    };
    let _ = writeln!(out, "uptime:         {:.2} s", uptime);
    let _ = writeln!(out, "syscalls total: {} ({:.0}/s avg)", total_calls, rate);
    let _ = writeln!(out);
    fmt_table(&mut out, rows);
    out
}

// ---------------------------------------------------------------------------
// Kernel runtime stats, surfaced at `/proc/perf/kernel` (heat / busy-spin
// debugging). Counters live in `kernel_hal::kstats`.
// ---------------------------------------------------------------------------

fn irq_note(vector: u16) -> &'static str {
    // LAPIC vectors use base 0xf0 on x86_64 (see kernel-hal x86_64 trap.rs).
    match vector {
        0xf0 => "LAPIC spurious",
        0xf1 => "LAPIC timer",
        0xf2 => "LAPIC error",
        _ => "",
    }
}

/// Render `/proc/perf/kernel`: idle vs busy, timer ticks and per-vector IRQs.
pub fn kernel_report() -> String {
    let ks = kernel_hal::kstats::snapshot();
    let (sched_polled, sched_weak) = kernel_hal::kstats::sched_stats();
    let uptime_ns = kernel_hal::timer::timer_now().as_nanos() as u64;
    let uptime_s = uptime_ns as f64 / 1e9;
    let total_cpus = kernel_hal::cpu::cpu_count().max(1) as u64;
    // Average busy% over the cores that actually came online, NOT the configured
    // CPU count: an AP that failed SMP bring-up never runs the idle loop, so
    // counting it in the denominator would charge its idle time as "busy" and
    // inflate the figure (a partial bring-up under QEMU/TCG is common). On a
    // healthy boot online == total and this is identical to before.
    let online_cpus = (kernel_hal::online_cpu_count() as u64).clamp(1, total_cpus);

    // idle% over a span: idle_ns against (span × online cpus) of capacity.
    fn pct_idle(idle_ns: u64, span_ns: u64, cpus: u64) -> f64 {
        let cap = span_ns.saturating_mul(cpus);
        if cap > 0 {
            (idle_ns as f64 * 100.0 / cap as f64).min(100.0)
        } else {
            0.0
        }
    }

    // ── Lifetime (since boot) figures ──
    let life_idle_pct = pct_idle(ks.idle_ns, uptime_ns, online_cpus);
    let life_busy_pct = (100.0 - life_idle_pct).max(0.0);
    let rate = |n: u64| {
        if uptime_s > 0.0 {
            n as f64 / uptime_s
        } else {
            0.0
        }
    };
    let life_cb_work_pct = if ks.idle_cb_total > 0 {
        ks.idle_cb_busy as f64 * 100.0 / ks.idle_cb_total as f64
    } else {
        0.0
    };

    // ── Windowed figures (delta since the previous read of this file) ──
    // A lifetime average is useless for "is the box busy *now*": on a long uptime
    // an early busy spell (boot, or a since-fixed busy-spin) keeps it pegged near
    // 100% even when every core is currently halted in `hlt`. The NMI probe sees
    // the truth (all cores caught at the post-`hlt` RIP) but the headline didn't.
    // Diff against the last snapshot so a second `cat` a moment later reports the
    // live figure. First read after boot has no previous sample and falls back to
    // the lifetime numbers.
    struct Prev {
        uptime_ns: u64,
        idle_ns: u64,
        idle_cb_total: u64,
        idle_cb_busy: u64,
        polled: u64,
        weak: u64,
        timer_ticks: u64,
    }
    static LAST: Mutex<Option<Prev>> = Mutex::new(None);
    let cur = Prev {
        uptime_ns,
        idle_ns: ks.idle_ns,
        idle_cb_total: ks.idle_cb_total,
        idle_cb_busy: ks.idle_cb_busy,
        polled: sched_polled,
        weak: sched_weak,
        timer_ticks: ks.timer_ticks,
    };
    let prev = LAST.lock().replace(cur);
    let win = prev.filter(|p| uptime_ns > p.uptime_ns).map(|p| {
        let d_ns = uptime_ns - p.uptime_ns;
        let d_s = d_ns as f64 / 1e9;
        let wr = move |now: u64, then: u64| {
            if d_s > 0.0 {
                now.saturating_sub(then) as f64 / d_s
            } else {
                0.0
            }
        };
        let idle_pct = pct_idle(ks.idle_ns.saturating_sub(p.idle_ns), d_ns, online_cpus);
        let d_cb_total = ks.idle_cb_total.saturating_sub(p.idle_cb_total);
        let cb_work_pct = if d_cb_total > 0 {
            ks.idle_cb_busy.saturating_sub(p.idle_cb_busy) as f64 * 100.0 / d_cb_total as f64
        } else {
            0.0
        };
        (
            d_s,
            idle_pct,
            (100.0 - idle_pct).max(0.0),
            cb_work_pct,
            wr(sched_polled, p.polled),
            wr(sched_weak, p.weak),
            wr(ks.timer_ticks, p.timer_ticks),
        )
    });
    // "Current" view drives the busy/attribution logic and the per-second rates;
    // falls back to lifetime on the first read after boot.
    let (win_s, idle_pct, busy_pct, cb_work_pct, polls_per_s, weak_per_s, timer_per_s, have_window) =
        match win {
            Some((s, i, b, cb, p, w, t)) => (s, i, b, cb, p, w, t, true),
            None => (
                0.0,
                life_idle_pct,
                life_busy_pct,
                life_cb_work_pct,
                rate(sched_polled),
                rate(sched_weak),
                rate(ks.timer_ticks),
                false,
            ),
        };

    let mut out = String::new();
    let _ = writeln!(out, "eclipse perf — kernel runtime stats");
    let _ = writeln!(out);
    match kernel_hal::cpu::cpu_temperature_mc() {
        Some(mc) => {
            let _ = writeln!(out, "cpu temp:     {}.{} C", mc / 1000, (mc % 1000) / 100);
        }
        None => {
            let _ = writeln!(out, "cpu temp:     n/a (no sensor or running in a VM)");
        }
    }
    // Adaptive P-state governor (bare metal only): shows whether any core is
    // currently throttled for heat, and cpu0's live ceiling vs its base.
    if let Some((throttled, ceiling, base)) = kernel_hal::cpu::pstate_governor_summary() {
        if throttled > 0 {
            let _ = writeln!(
                out,
                "pstate gov:   {} core(s) throttling — cpu0 ceiling {}/{}",
                throttled, ceiling, base
            );
        } else {
            let _ = writeln!(
                out,
                "pstate gov:   nominal — cpu0 ceiling {}/{}",
                ceiling, base
            );
        }
    }
    let _ = writeln!(
        out,
        "uptime:       {:.2} s   cpus: {} online ({} configured)",
        uptime_s, online_cpus, total_cpus
    );
    if have_window {
        let _ = writeln!(
            out,
            "cpu idle:     {:.1}%   busy: {:.1}%   (last {:.2}s window over {} online cpu(s))",
            idle_pct, busy_pct, win_s, online_cpus
        );
        let _ = writeln!(
            out,
            "  lifetime:   idle {:.1}%   busy {:.1}%   (since boot — skewed by past load)",
            life_idle_pct, life_busy_pct
        );
    } else {
        let _ = writeln!(
            out,
            "cpu idle:     {:.1}%   busy: {:.1}%   (lifetime since boot — run `cat` again for a recent-window figure)",
            idle_pct, busy_pct
        );
    }
    let avg_nap_us = if ks.idle_entries > 0 {
        ks.idle_ns as f64 / ks.idle_entries as f64 / 1000.0
    } else {
        0.0
    };
    let _ = writeln!(
        out,
        "idle naps:    {}  (avg {:.1} us/nap, lifetime)",
        ks.idle_entries, avg_nap_us
    );
    // [diag] Busy attribution — kept HIGH in the report, *before* the per-CPU
    // breakdowns. With many cores those lists are 20+ lines each and push the
    // attribution counters (idle-callback %, sched polls, tick ctx, NMI rip) past
    // the first screenful, so a `head -30` or a phone photo of the console shows
    // 100% busy with no hint of *why*. This single line names the dominant
    // non-halting path so even a truncated capture localises the spin; the
    // per-CPU tick ctx (%user) and NMI rip lines below pin it exactly. All the
    // figures here are the windowed ones (or lifetime on the first read), so the
    // verdict matches what the box is doing *now*, not its lifetime average.
    // Aggregate ring-3 share of scheduler ticks across all cores: a high figure
    // on a pegged box means a *user* thread is busy-spinning (a runaway process);
    // a low figure points the spin at kernel code (a poll/lock loop).
    let (tick_user, tick_total) = ks
        .tick_percpu
        .iter()
        .fold((0u64, 0u64), |(u, t), (_, total, user, _)| {
            (u + *user, t + *total)
        });
    let user_pct = if tick_total > 0 {
        tick_user as f64 * 100.0 / tick_total as f64
    } else {
        0.0
    };
    // Off-scheduler wedge detector: the box is pegged but *every* scheduler/idle
    // counter is ~zero AND timers barely tick. That means the busy time is spent
    // OUTSIDE the run loop entirely — almost always a spin with interrupts
    // disabled (a kernel livelock/deadlock), which also explains the stalled
    // timer. Only the NMI probe (delivered even with IRQs off) can see it, so
    // capture it now and surface the spin RIPs up here — the full per-CPU probe
    // is far below the per-core lists, off the first screenful.
    let scheduler_quiet = cb_work_pct < 1.0 && weak_per_s < 1.0 && polls_per_s < 1.0;
    // Only trust the wedge verdict on a real measurement window. On the first
    // read there is no window, so every figure here is the *lifetime* average:
    // on a long-uptime box that means busy% near 100 (skewed by past load) while
    // the lifetime poll/weak/timer rates round to ~0/s — which trips this
    // detector and screams "kernel livelock" at a perfectly idle machine. The
    // NMI probe then catches every core at its idle `hlt` RIP and mislabels them
    // as wedge sites. Require `have_window` so the verdict reflects *now*.
    let off_sched_wedge = have_window && busy_pct > 50.0 && scheduler_quiet && timer_per_s < 50.0;
    kernel_hal::kstats::capture_cpu_rips();
    let nmi = kernel_hal::kstats::nmi_rips();
    // How many cores are parked in idle `hlt` *right now* (robust per-CPU flag set
    // around enable_and_hlt — not a build-dependent RIP match). The core rendering
    // this report is itself busy, so a fully-idle box shows online-1 here. If
    // essentially every other core is halted, a high busy% is a lifetime-average
    // artifact, not a live spin.
    let idle_now = kernel_hal::kstats::cpus_idle_now() as u64;
    let all_halted = online_cpus > 1 && idle_now >= online_cpus - 1;
    let _ = writeln!(
        out,
        "cores idle now: {}/{} parked in hlt this instant",
        idle_now, online_cpus
    );
    let suspect = if !have_window {
        // First read after boot (or after a long gap with no prior sample): only
        // the lifetime average is available, which says nothing about what the
        // CPUs are doing *now*. Don't run the live heuristics on it — the
        // OFF-SCHEDULER wedge detector in particular produces a false "kernel
        // livelock" verdict here. Defer to the next read (which has a window).
        "unknown — first read has only the lifetime average; \
         run `cat` again for a live (windowed) verdict"
    } else if busy_pct < 50.0 {
        "none — cores mostly reach halt"
    } else if all_halted {
        "NONE NOW — every core is halted in hlt this instant; high busy% is a \
         lifetime-average artifact (read again for the windowed figure above)"
    } else if cb_work_pct > 50.0 {
        "deferred-job drain — idle callback keeps finding work, cores never halt"
    } else if weak_per_s > 100.0 {
        "weak-executor yields — long futures preempted, scheduler re-spins (kernel)"
    } else if user_pct > 60.0 {
        "user thread busy-spin — a process is pegging the cpu (see tick ctx below)"
    } else if polls_per_s > 5000.0 {
        "task busy-poll — a coroutine re-polled without ever sleeping (kernel)"
    } else if off_sched_wedge {
        "OFF-SCHEDULER spin — pegged with no run-loop activity and timers stalled: \
         interrupts-disabled kernel livelock (see wedge rips)"
    } else {
        "unclear — read tick ctx (%user) and nmi probe rip below"
    };
    let _ = writeln!(out, "busy attribution: {}", suspect);
    let _ = writeln!(
        out,
        "  (idle-cb work {:.0}%, weak-yield {:.0}/s, task-polls {:.0}/s, tick {:.0}% user, timer {:.0}/s{})",
        cb_work_pct, weak_per_s, polls_per_s, user_pct, timer_per_s,
        if have_window { "" } else { " — lifetime" }
    );
    if off_sched_wedge && !all_halted && !nmi.is_empty() {
        // Distinct current RIPs across cores. A single shared value means every
        // wedged core is stuck at the same spin site (one lock / one loop);
        // resolve it with addr2line against the kernel image.
        let mut rips: Vec<u64> = nmi.iter().map(|(_, r)| *r).collect();
        rips.sort_unstable();
        rips.dedup();
        let _ = write!(out, "  wedge rips ({} core(s),", nmi.len());
        let _ = write!(out, " {} distinct):", rips.len());
        for r in rips.iter().take(6) {
            let _ = write!(out, " {:#x}", r);
        }
        if rips.len() > 6 {
            let _ = write!(out, " (+{} more)", rips.len() - 6);
        }
        let _ = writeln!(out);
    }
    // [diag] Per-CPU nap breakdown: a core driving the HID poll should show many
    // short naps (low avg us); a deeply-idle core shows few long naps.
    for (cpu, naps, ns) in &ks.idle_percpu {
        let avg_us = if *naps > 0 {
            *ns as f64 / *naps as f64 / 1000.0
        } else {
            0.0
        };
        let _ = writeln!(
            out,
            "  cpu{}: {} naps ({:.0}/s), avg {:.0} us/nap",
            cpu,
            naps,
            rate(*naps),
            avg_us
        );
    }
    // [diag] Per-CPU tick context: of the scheduler ticks that hit each core, how
    // many interrupted user mode (a ring-3 thread burning CPU) vs kernel mode. A
    // pegged core with a high user% is running a CPU-bound user thread; a high
    // kernel% on a pegged core points at a kernel-side spin (lock / poll loop).
    if !ks.tick_percpu.is_empty() {
        let _ = writeln!(out, "tick ctx (user/total, last rip):");
        for (cpu, total, user, rip) in &ks.tick_percpu {
            let pct = if *total > 0 {
                *user as f64 * 100.0 / *total as f64
            } else {
                0.0
            };
            let _ = writeln!(
                out,
                "  cpu{}: {}/{} ({:.0}% user) rip={:#x}",
                cpu, user, total, pct, rip
            );
        }
    }
    // [diag] NMI probe: interrupt every other CPU (delivered even with IRQs off)
    // and report its *current* RIP. For a core wedged in an interrupts-disabled
    // spin this is the actual spin site — resolve with addr2line. `nmi` was
    // captured once up in the busy-attribution block; reuse it here.
    if !nmi.is_empty() {
        let _ = writeln!(out, "nmi probe (current rip per cpu):");
        for (cpu, rip) in &nmi {
            let _ = writeln!(out, "  cpu{}: {:#x}", cpu, rip);
        }
    }
    // [diag] xHCI HID poll rate by path. Input is delivered from these polls;
    // when idle, `iowait` falls to ~0 and `timer` alone must keep input alive.
    let _ = writeln!(
        out,
        "hid polls:    timer {} ({:.0}/s), iowait {} ({:.0}/s)",
        ks.hid_poll_timer,
        rate(ks.hid_poll_timer),
        ks.hid_poll_iowait,
        rate(ks.hid_poll_iowait)
    );
    let _ = writeln!(
        out,
        "timer ticks:  {}  ({:.0}/s)",
        ks.timer_ticks,
        rate(ks.timer_ticks)
    );
    let _ = writeln!(
        out,
        "interrupts:   {}  ({:.0}/s)",
        ks.irq_total,
        rate(ks.irq_total)
    );
    // Idle-callback hit rate: the scheduler only halts when this finds no
    // deferred work, so a high "had work" share means the CPUs busy-spin
    // draining jobs (the heat signature) rather than sleeping. `cb_work_pct` is
    // computed above for the busy-attribution summary.
    let _ = writeln!(
        out,
        "idle callback: {} calls ({:.0}/s), {:.1}% found deferred work",
        ks.idle_cb_total,
        rate(ks.idle_cb_total),
        cb_work_pct
    );
    let _ = writeln!(
        out,
        "deferred jobs pending now: {}",
        kernel_hal::deferred_job::pending_deferred_jobs()
    );
    // `sched_polled`/`sched_weak` were sampled above for the attribution summary.
    let _ = writeln!(
        out,
        "sched: {} task polls ({:.0}/s), {} weak-exec yields ({:.0}/s)",
        sched_polled, polls_per_s, sched_weak, weak_per_s
    );
    // Which scheduler/timer mode this boot is running in, so a captured report
    // is self-describing when compared against another.
    {
        let (deadline, wakeup) = kernel_hal::kstats::sched_switches();
        let _ = writeln!(
            out,
            "sched mode:   deadline-timer={} wakeup-preempt={} cow-fork={} fork-gather={}",
            if deadline { "on" } else { "OFF" },
            if wakeup { "on" } else { "OFF" },
            if zircon_object::vm::cow_fork_enabled() {
                "on"
            } else {
                "OFF"
            },
            if zircon_object::vm::fork_gather_enabled() {
                "on"
            } else {
                "OFF"
            },
        );
    }
    // Copy-on-write tree census. A fork inserts a hidden node above every
    // mapping's VMO and the child's exit should collapse it again; `hidden`
    // failing to return to its pre-fork value is a tree that did not, and that
    // is measurable from userspace by reading this around a `fork`.
    {
        let (paged, hidden, snapshots) = zircon_object::vm::cow_tree_stats();
        let _ = writeln!(
            out,
            "cow tree:     {} paged vmos live, {} hidden live, {} snapshots taken",
            paged, hidden, snapshots
        );
    }
    // Where a fork's time actually goes, per mapping cloned.
    {
        let (n, total, create, protect, committed, allocs) =
            zircon_object::vm::fork_phase_stats();
        if n > 0 {
            let us = |v: u64| v as f64 / 1000.0 / n as f64;
            let _ = writeln!(
                out,
                "fork phases:  {} mappings cloned, {:.1} us each (create_child {:.1}, \
                 protect {:.1}, map_committed {:.1}, rest {:.1}), {:.1} allocs each",
                n,
                us(total),
                us(create),
                us(protect),
                us(committed),
                us(total.saturating_sub(create + protect)),
                allocs as f64 / n as f64,
            );
        }
    }
    // Kernel heap profile (only populated with `HEAPPROF=1`). A `dealloc`
    // average far above `alloc`, and one that climbs with the number of live
    // same-sized objects, is the buddy allocator's linear free-list scan.
    {
        let (ac, acy, dc, dcy) = kernel_hal::kstats::heap_prof_stats();
        if ac > 0 || dc > 0 {
            let _ = writeln!(
                out,
                "heap prof:    alloc {} calls, {} cyc avg; dealloc {} calls, {} cyc avg",
                ac,
                if ac > 0 { acy / ac } else { 0 },
                dc,
                if dc > 0 { dcy / dc } else { 0 },
            );
        }
    }
    // The per-VMO mapping list a fork walks once per mapping.
    {
        let (scans, entries, dead, max) = zircon_object::vm::mapping_list_stats();
        if scans > 0 {
            let _ = writeln!(
                out,
                "vmo maplist:  {} scans, {:.1} entries avg, {} dead, {} longest",
                scans,
                entries as f64 / scans as f64,
                dead,
                max
            );
        }
    }
    // The eager-copy fallback: mappings a fork could not share.
    {
        let (n, bytes, ns) = zircon_object::vm::fork_eager_stats();
        let _ = writeln!(
            out,
            "fork eager:   {} mappings copied ({} KiB, {:.1} ms total)",
            n,
            bytes / 1024,
            ns as f64 / 1e6,
        );
    }
    // vDSO state. Three things can independently stop `clock_gettime` from
    // being answered in userspace — the build had no C compiler, the image
    // could not be placed in physical memory, or the TSC is not fit to be read
    // directly — and all three degrade silently into "the syscall is still
    // taken". This is where a boot says which, without needing a bisect.
    {
        let _ = writeln!(out, "vdso:         {}", crate::vdso::status());
    }
    // Deadline timer: re-arms against ticks. The timer is programmed for the
    // nearest pending deadline instead of rounding every sleep/poll timeout up
    // to the 4 ms scheduler tick; this is the sanity check that it is buying
    // precision rather than degenerating into an interrupt storm.
    {
        let per_tick = if ks.timer_ticks > 0 {
            ks.timer_rearms as f64 / ks.timer_ticks as f64
        } else {
            0.0
        };
        let _ = writeln!(
            out,
            "timer rearms: {} ({:.0}/s, {:.2} per tick)",
            ks.timer_rearms,
            rate(ks.timer_rearms),
            per_tick
        );
    }
    // Wake-up preemption: how often a task became runnable on a CPU that was
    // busy with someone else, and how often that actually cut the running
    // thread's timeslice short. Without this the woken task waits out the full
    // slice (up to 20 ms) — invisible to single-threaded benchmarks, very
    // visible when using the machine.
    {
        let (req, taken) = kernel_hal::kstats::wakeup_preempt_stats();
        let pct = if req > 0 {
            taken as f64 * 100.0 / req as f64
        } else {
            0.0
        };
        let _ = writeln!(
            out,
            "wakeup preempt: {} requests ({:.0}/s), {} honoured ({:.1}%)",
            req,
            rate(req),
            taken,
            pct
        );
    }
    let _ = writeln!(out);
    if busy_pct > 50.0 {
        let _ = writeln!(
            out,
            "note: CPUs are busy >50% while you read this — if the system looks idle,",
        );
        let _ = writeln!(
            out,
            "      something is busy-spinning (a likely source of heat). The busiest",
        );
        let _ = writeln!(
            out,
            "      IRQ vectors below hint at runaway interrupt sources."
        );
        let _ = writeln!(out);
    }
    let _ = writeln!(
        out,
        "  {:>8}  {:>12}  {:>10}  NOTE",
        "VECTOR", "COUNT", "PER SEC"
    );
    let mut irqs = ks.irqs;
    irqs.sort_by_key(|r| core::cmp::Reverse(r.1));
    for (v, c) in irqs {
        let _ = writeln!(
            out,
            "  {:>#8x}  {:>12}  {:>10.0}  {}",
            v,
            c,
            rate(c),
            irq_note(v)
        );
    }
    out
}

// ---------------------------------------------------------------------------
// Sampling profiler ("our own perf top"), surfaced at `/proc/perf/top`.
// ---------------------------------------------------------------------------

/// Cluster sampled instruction pointers into 64-byte buckets so a hot function
/// aggregates instead of scattering across every instruction.
const PC_BUCKET: u64 = 64;
/// Cap on distinct buckets tracked, to bound memory and lock time. Once full,
/// new addresses are counted as "dropped" rather than inserted.
const TOP_MAX: usize = 4096;

struct SampleState {
    total: u64,
    dropped: u64,
    map: BTreeMap<u64, u64>,
}

static SAMPLES: Mutex<SampleState> = Mutex::new(SampleState {
    total: 0,
    dropped: 0,
    map: BTreeMap::new(),
});

/// Per-timer-tick hook: record one user-space sample and forward it to any
/// active Linux-`perf` ring buffer. Cheap; called from the timer-interrupt
/// return path while a user thread was running.
pub fn tick(pid: i32, tid: i32, cpu: u32, pc: u64) {
    sample_pc(pc);
    crate::fs::perf_sample_user(pid, tid, cpu, pc);
}

/// Add one instruction-pointer sample to the global histogram.
fn sample_pc(pc: u64) {
    if pc == 0 {
        return;
    }
    let key = pc & !(PC_BUCKET - 1);
    let mut s = SAMPLES.lock();
    s.total += 1;
    if let Some(c) = s.map.get_mut(&key) {
        *c += 1;
    } else if s.map.len() < TOP_MAX {
        s.map.insert(key, 1);
    } else {
        s.dropped += 1;
    }
}

/// Render `/proc/perf/top`: hottest sampled instruction-pointer buckets.
pub fn top_report() -> String {
    let (total, dropped, mut rows) = {
        let s = SAMPLES.lock();
        let rows: Vec<(u64, u64)> = s.map.iter().map(|(&k, &v)| (k, v)).collect();
        (s.total, s.dropped, rows)
    };
    rows.sort_by_key(|r| core::cmp::Reverse(r.1));

    let mut out = String::new();
    let _ = writeln!(out, "eclipse perf — sampled CPU profile (user space)");
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "samples: {}   tracked buckets: {}   dropped (table full): {}",
        total,
        rows.len(),
        dropped
    );
    let _ = writeln!(out, "bucket granularity: {} bytes", PC_BUCKET);
    let _ = writeln!(out);
    if total == 0 {
        let _ = writeln!(
            out,
            "(no samples yet — a user process must be running on a timer tick)"
        );
        return out;
    }
    let _ = writeln!(
        out,
        "  {:>7}  {:>10}  {:<18}",
        "OVERHEAD", "SAMPLES", "IP (bucket)"
    );
    for (addr, count) in rows.into_iter().take(40) {
        let pct = count as f64 * 100.0 / total as f64;
        let _ = writeln!(out, "  {:>6.2}%  {:>10}  {:#018x}", pct, count, addr);
    }
    out
}

/// Render `/proc/<pid>/perf`: one process's syscall accounting.
pub fn proc_report(proc: &LinuxProcess, pid: u64) -> String {
    let perf = proc.perf();
    let (total, ns) = perf.totals();
    let mut rows: Vec<(u32, u64, u64)> = Vec::new();
    for i in 0..perf.per.len() {
        let calls = perf.per[i].load(Relaxed) as u64;
        if calls != 0 {
            rows.push((i as u32, calls, perf.per_ns[i].load(Relaxed)));
        }
    }
    let mut out = String::new();
    let path = proc.execute_path();
    let name = if path.is_empty() { "?" } else { path.as_str() };
    let _ = writeln!(out, "eclipse perf — pid {} ({})", pid, name);
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "syscalls total: {}   time in syscalls: {:.3} ms",
        total,
        ns as f64 / 1_000_000.0
    );
    let _ = writeln!(out);
    fmt_table(&mut out, rows);
    out
}
