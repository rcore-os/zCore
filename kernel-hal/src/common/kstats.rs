//! Lightweight kernel runtime statistics, surfaced at `/proc/perf/kernel`.
//!
//! Aimed at debugging "why is the machine warm / busy": the headline numbers
//! are **how much wall-clock the CPUs actually spent halted (idle)** vs running,
//! and **the per-vector interrupt counts** (an IRQ storm is the usual culprit
//! behind unexpected heat). Everything here is lock-free atomic counters bumped
//! from the bare-metal idle / IRQ / timer paths; on libos they stay zero.

use crate::config::MAX_CORE_NUM;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};

/// Number of interrupt vectors tracked individually (x86 IDT is 256 wide).
const NVEC: usize = 256;

static IDLE_NS: AtomicU64 = AtomicU64::new(0);
static IDLE_ENTRIES: AtomicU64 = AtomicU64::new(0);

/// [diag] Per-CPU idle nap accounting (ns halted, and nap count), indexed by the
/// dense logical CPU id. Shows whether a *specific* core keeps a short idle cap
/// (frequent short naps → it is the one driving the background HID poll) or
/// sleeps long (deep idle). Used to debug input-responsiveness-vs-heat.
static IDLE_NS_PERCPU: [AtomicU64; MAX_CORE_NUM] = [const { AtomicU64::new(0) }; MAX_CORE_NUM];
static IDLE_ENTRIES_PERCPU: [AtomicU64; MAX_CORE_NUM] = [const { AtomicU64::new(0) }; MAX_CORE_NUM];

/// [diag] xHCI HID poll invocations split by the path that issued them: the
/// timer tick (`timer`) vs an I/O-wait loop (`iowait`). Keyboard/mouse input is
/// delivered from these polls, so their combined rate IS the input
/// responsiveness. When the CPUs halt and the net busy-spin is gone, `iowait`
/// drops to ~0 and only `timer` keeps input alive — its rate then says whether
/// idle HID polling is fast enough.
static HID_POLL_TIMER: AtomicU64 = AtomicU64::new(0);
static HID_POLL_IOWAIT: AtomicU64 = AtomicU64::new(0);

/// [diag] Account one xHCI HID poll issued from the timer tick.
pub fn note_hid_poll_timer() {
    HID_POLL_TIMER.fetch_add(1, Relaxed);
}

/// [diag] Account one xHCI HID poll issued from an I/O-wait loop.
pub fn note_hid_poll_iowait() {
    HID_POLL_IOWAIT.fetch_add(1, Relaxed);
}
static TIMER_TICKS: AtomicU64 = AtomicU64::new(0);
/// LAPIC deadline re-arms: how often the timer was reprogrammed to fire sooner
/// than the scheduler tick would have. Compared against `timer_ticks` it says
/// whether deadline programming is buying precision (a handful of re-arms per
/// tick) or has degenerated into an interrupt storm (re-arms >> ticks).
static TIMER_REARMS: AtomicU64 = AtomicU64::new(0);
static IRQ_TOTAL: AtomicU64 = AtomicU64::new(0);
static IRQ_COUNTS: [AtomicU64; NVEC] = [const { AtomicU64::new(0) }; NVEC];
/// Idle-callback invocations and how many found deferred work pending. The
/// scheduler only halts when the callback finds nothing, so a high "busy" ratio
/// here means the idle path keeps finding work and the CPUs never sleep — the
/// signature of a busy-spin (and the heat that comes with it).
static IDLE_CB_TOTAL: AtomicU64 = AtomicU64::new(0);
static IDLE_CB_BUSY: AtomicU64 = AtomicU64::new(0);

/// Per-CPU user-/kernel-mode nanoseconds, attributed by `ThreadSwitchFuture::poll`
/// (zircon-object) from the wall-clock span of one `Future::poll` call — the
/// only span that can never include time a thread spent blocked, since `poll`
/// is synchronous and cannot itself await. These back the `user`/`system`
/// columns of `/proc/stat` and (summed with `IDLE_NS_PERCPU`) let `idle` there
/// agree with the halt-time-based figure `/proc/perf/kernel` already reports.
static USER_NS_PERCPU: [AtomicU64; MAX_CORE_NUM] = [const { AtomicU64::new(0) }; MAX_CORE_NUM];
static SYS_NS_PERCPU: [AtomicU64; MAX_CORE_NUM] = [const { AtomicU64::new(0) }; MAX_CORE_NUM];

/// Attribute `ns` nanoseconds of user-mode execution to `cpu`.
#[inline(always)]
pub fn note_user_time(cpu: usize, ns: u64) {
    if cpu < MAX_CORE_NUM {
        USER_NS_PERCPU[cpu].fetch_add(ns, Relaxed);
    }
}

/// Attribute `ns` nanoseconds of kernel-mode execution to `cpu`.
#[inline(always)]
pub fn note_sys_time(cpu: usize, ns: u64) {
    if cpu < MAX_CORE_NUM {
        SYS_NS_PERCPU[cpu].fetch_add(ns, Relaxed);
    }
}

/// `(user, system, idle)` for one logical CPU, in USER_HZ (100 Hz) jiffies —
/// the unit `/proc/stat` reports. Reads zero for a CPU that never came online
/// (or past `MAX_CORE_NUM`), matching Linux's own "absent means zero" reading.
pub fn cpu_times_jiffies(cpu: usize) -> (u64, u64, u64) {
    const NS_PER_JIFFY: u64 = 10_000_000; // 1e7 ns = 10 ms = 1 / USER_HZ(100)
    if cpu >= MAX_CORE_NUM {
        return (0, 0, 0);
    }
    let user = USER_NS_PERCPU[cpu].load(Relaxed) / NS_PER_JIFFY;
    let sys = SYS_NS_PERCPU[cpu].load(Relaxed) / NS_PER_JIFFY;
    let idle = IDLE_NS_PERCPU[cpu].load(Relaxed) / NS_PER_JIFFY;
    (user, sys, idle)
}

/// `(tasks polled, weak-executor yields)` from the scheduler loop — to attribute
/// a busy-spin: a high `polled` rate means a task keeps re-readying itself; a
/// high `weak_yield` rate means the CPUs spin on an outstanding weak executor.
/// (The scheduler only exists on bare metal; libos reports zeros.)
#[cfg(target_os = "none")]
pub fn sched_stats() -> (u64, u64) {
    executor::sched_stats()
}

#[cfg(not(target_os = "none"))]
pub fn sched_stats() -> (u64, u64) {
    (0, 0)
}

/// `(deadline timer enabled, wake-up preemption enabled)`.
///
/// Both are boot switches (`TIMERDEADLINE=0` / `WAKEPREEMPT=0`). Reporting them
/// means a captured `/proc/perf/kernel` says which mode produced it — an A/B
/// pair of logs that does not record its own configuration is two numbers with
/// nothing tying them to a cause.
#[cfg(target_os = "none")]
pub fn sched_switches() -> (bool, bool) {
    (
        crate::timer::deadline_timer_enabled(),
        executor::wakeup_preempt_enabled(),
    )
}

#[cfg(not(target_os = "none"))]
pub fn sched_switches() -> (bool, bool) {
    (false, false)
}

/// `(wake-up preemption requests, requests honoured)`.
///
/// A request is raised when a task becomes runnable on a CPU that is busy with
/// a different task; it is honoured when that CPU's user-trap path yields in
/// response. `honoured / requested` is the share of wakes that actually cut
/// short someone else's timeslice instead of waiting it out — the interactive
/// latency knob. A large shortfall means the requests are landing on CPUs that
/// stay in kernel mode, where the trap path never sees them.
#[cfg(target_os = "none")]
pub fn wakeup_preempt_stats() -> (u64, u64) {
    executor::wakeup_preempt_stats()
}

#[cfg(not(target_os = "none"))]
pub fn wakeup_preempt_stats() -> (u64, u64) {
    (0, 0)
}

/// Account one idle-callback invocation; `had_work` is whether it found deferred
/// jobs (and so kept the CPU from halting).
pub fn note_idle_callback(had_work: bool) {
    IDLE_CB_TOTAL.fetch_add(1, Relaxed);
    if had_work {
        IDLE_CB_BUSY.fetch_add(1, Relaxed);
    }
}

/// Account `ns` of wall-clock spent halted in one idle nap. Called by the
/// per-CPU idle routine around its `hlt`/`mwait`.
pub fn note_idle(ns: u64) {
    IDLE_NS.fetch_add(ns, Relaxed);
    IDLE_ENTRIES.fetch_add(1, Relaxed);
    // [diag] per-CPU breakdown (cpu_id is 0 on libos, real on bare).
    let cpu = crate::cpu::cpu_id() as usize;
    if cpu < MAX_CORE_NUM {
        IDLE_NS_PERCPU[cpu].fetch_add(ns, Relaxed);
        IDLE_ENTRIES_PERCPU[cpu].fetch_add(1, Relaxed);
    }
}

/// [diag] Whether each logical CPU is *currently* parked in its idle `hlt`/
/// `mwait`. Set immediately before the halt and cleared right after it wakes, so
/// a reader can tell a genuinely-idle core (halted now) from a busy-spinning one
/// — the distinction the lifetime busy% average cannot make. This is the robust,
/// build-independent version of "is the captured RIP the post-`hlt` instruction".
static CPU_IN_IDLE: [AtomicBool; MAX_CORE_NUM] = [const { AtomicBool::new(false) }; MAX_CORE_NUM];

/// Mark the calling CPU as entering (`true`) or leaving (`false`) idle halt.
pub fn set_cpu_idle(in_idle: bool) {
    let cpu = crate::cpu::cpu_id() as usize;
    if cpu < MAX_CORE_NUM {
        CPU_IN_IDLE[cpu].store(in_idle, Relaxed);
    }
}

/// Number of logical CPUs currently parked in idle halt (best-effort snapshot).
pub fn cpus_idle_now() -> usize {
    CPU_IN_IDLE.iter().filter(|c| c.load(Relaxed)).count()
}

/// Whether the *calling* CPU is currently marked idle-halted. Meaningful from
/// an interrupt handler: the flag is set by the idle path around its `hlt`, so
/// an IRQ that reads `true` interrupted the halt itself.
pub fn current_cpu_in_idle() -> bool {
    let cpu = crate::cpu::cpu_id() as usize;
    cpu < MAX_CORE_NUM && CPU_IN_IDLE[cpu].load(Relaxed)
}

/// The `{data, vtable}` words of the timer callback this CPU is currently
/// invoking (0,0 = none). Published by `timer_tick` around each dispatch so a
/// fault *inside* a callback can name the exact closure — the vtable
/// symbolizes to the closure type of the `timer_set` call site.
static TIMER_CB_DATA: [AtomicU64; MAX_CORE_NUM] = [const { AtomicU64::new(0) }; MAX_CORE_NUM];
static TIMER_CB_VTABLE: [AtomicU64; MAX_CORE_NUM] = [const { AtomicU64::new(0) }; MAX_CORE_NUM];

/// Record the callback about to run on this CPU (0,0 clears).
pub fn note_timer_cb(data: u64, vtable: u64) {
    let cpu = crate::cpu::cpu_id() as usize;
    if cpu < MAX_CORE_NUM {
        TIMER_CB_DATA[cpu].store(data, Relaxed);
        TIMER_CB_VTABLE[cpu].store(vtable, Relaxed);
    }
}

/// `(data, vtable)` of the timer callback the calling CPU is inside, or (0,0).
pub fn current_timer_cb() -> (u64, u64) {
    let cpu = crate::cpu::cpu_id() as usize;
    if cpu < MAX_CORE_NUM {
        (
            TIMER_CB_DATA[cpu].load(Relaxed),
            TIMER_CB_VTABLE[cpu].load(Relaxed),
        )
    } else {
        (0, 0)
    }
}

/// Bitmask of logical CPUs currently parked in idle halt (bit `i` = cpu `i`).
/// Used by the TLB-shootdown initiator to avoid synchronously waiting on a core
/// that is halted: a halted core is not executing, and the shootdown IPI it was
/// sent will flush its TLB when it wakes (before it runs any user instruction),
/// exactly as the existing budget-exhaustion fire-and-forget fallback relies on.
pub fn cpu_idle_mask() -> u64 {
    let mut mask = 0u64;
    for (i, c) in CPU_IN_IDLE.iter().enumerate() {
        if i < 64 && c.load(Relaxed) {
            mask |= 1u64 << i;
        }
    }
    mask
}

/// Account one timer tick.
pub fn note_timer_tick() {
    TIMER_TICKS.fetch_add(1, Relaxed);
}

/// Account one LAPIC deadline re-arm.
pub fn note_timer_rearm() {
    TIMER_REARMS.fetch_add(1, Relaxed);
}

// ---------------------------------------------------------------------------
// Kernel heap profiling
// ---------------------------------------------------------------------------
//
// `fork` was measured at 471.7 us per mapping with 512 mappings against 55.8 us
// with 32 — quadratic in the mapping count, degrading `create_child` and the
// rest of `clone_map` together, and resetting when the process is replaced. That
// pattern points beneath both to something every one of them does, and the
// suspect is the buddy allocator: `buddy_system_allocator` 0.8.0's `dealloc`
// finds a block's buddy by scanning the whole free list of its size class,
// repeating for each class it merges up through. A fork of n mappings creates
// and destroys about 3n objects of the same size, so the list grows to ~n and
// each free costs O(n).
//
// Timing that is the confirmation, and it has to be paid for carefully: this is
// the hottest path in the kernel. Hence a switch (`HEAPPROF=1`), off by default,
// and raw `rdtsc` rather than `timer_now()` — which on a machine whose TSC is
// not invariant does a `fetch_max` on a globally shared cacheline and would
// swamp the very cost being measured. Cycles, not nanoseconds: only the ratio
// between configurations matters here, and converting would need the very clock
// this avoids.
static HEAP_PROF: AtomicBool = AtomicBool::new(false);
static HEAP_ALLOC_CALLS: AtomicU64 = AtomicU64::new(0);
static HEAP_ALLOC_CYCLES: AtomicU64 = AtomicU64::new(0);
static HEAP_DEALLOC_CALLS: AtomicU64 = AtomicU64::new(0);
static HEAP_DEALLOC_CYCLES: AtomicU64 = AtomicU64::new(0);

/// Enable heap profiling (`HEAPPROF=1` on the kernel command line).
pub fn set_heap_prof(on: bool) {
    HEAP_PROF.store(on, Relaxed);
}

/// Whether the allocator should time itself. Read once per allocation, so it is
/// a single relaxed load on the hot path when off.
#[inline(always)]
pub fn heap_prof_enabled() -> bool {
    HEAP_PROF.load(Relaxed)
}

/// Account one allocation taking `cycles` (0 when profiling is off).
///
/// The *count* is kept unconditionally — one relaxed add, alongside the two the
/// allocator already does — because it is what attributes allocations to a
/// caller: sampling it around a region of code gives that region's allocation
/// count without any per-caller plumbing. The cycle total is only meaningful
/// with `HEAPPROF=1`.
#[inline(always)]
pub fn note_heap_alloc(cycles: u64) {
    HEAP_ALLOC_CALLS.fetch_add(1, Relaxed);
    if cycles != 0 {
        HEAP_ALLOC_CYCLES.fetch_add(cycles, Relaxed);
    }
}

/// Allocations performed since boot. Sample around a region to count its own.
#[inline(always)]
pub fn heap_alloc_calls() -> u64 {
    HEAP_ALLOC_CALLS.load(Relaxed)
}

/// Account one deallocation taking `cycles`.
#[inline(always)]
pub fn note_heap_dealloc(cycles: u64) {
    HEAP_DEALLOC_CALLS.fetch_add(1, Relaxed);
    if cycles != 0 {
        HEAP_DEALLOC_CYCLES.fetch_add(cycles, Relaxed);
    }
}

/// `(alloc calls, alloc cycles, dealloc calls, dealloc cycles)`.
///
/// A `dealloc` average that climbs with the number of live same-sized objects,
/// while `alloc` stays flat, is the free-list scan.
pub fn heap_prof_stats() -> (u64, u64, u64, u64) {
    (
        HEAP_ALLOC_CALLS.load(Relaxed),
        HEAP_ALLOC_CYCLES.load(Relaxed),
        HEAP_DEALLOC_CALLS.load(Relaxed),
        HEAP_DEALLOC_CYCLES.load(Relaxed),
    )
}

/// [diag] Per-CPU timer ticks, split by whether the tick interrupted user mode
/// (a thread burning CPU in ring 3) or kernel mode (idle `hlt`, a syscall, or a
/// kernel busy-spin). A core that is pegged with mostly *user* ticks is running
/// a CPU-bound user thread; mostly *kernel* ticks on a pegged core points at a
/// kernel-side spin (lock / poll loop). Used to locate the source of idle heat.
static TICK_TOTAL_PERCPU: [AtomicU64; MAX_CORE_NUM] = [const { AtomicU64::new(0) }; MAX_CORE_NUM];
static TICK_USER_PERCPU: [AtomicU64; MAX_CORE_NUM] = [const { AtomicU64::new(0) }; MAX_CORE_NUM];
/// [diag] Most recent RIP observed when a tick interrupted this CPU. For a core
/// wedged in an interrupts-off spin (no more ticks) this stays frozen at the RIP
/// it had on its last tick — i.e. near where it entered the spin — so it can be
/// resolved to a symbol with addr2line.
static TICK_LAST_RIP_PERCPU: [AtomicU64; MAX_CORE_NUM] =
    [const { AtomicU64::new(0) }; MAX_CORE_NUM];

/// [diag] Account one timer tick by the context it interrupted, recording the
/// interrupted instruction pointer.
pub fn note_tick_context(from_user: bool, rip: u64) {
    let cpu = crate::cpu::cpu_id() as usize;
    if cpu < MAX_CORE_NUM {
        TICK_TOTAL_PERCPU[cpu].fetch_add(1, Relaxed);
        if from_user {
            TICK_USER_PERCPU[cpu].fetch_add(1, Relaxed);
        }
        TICK_LAST_RIP_PERCPU[cpu].store(rip, Relaxed);
    }
}

/// [diag] The RIP a timer tick last observed interrupting THIS cpu — a
/// best-effort "what was running here" for the isolation heuristic. A low value
/// (`< 0xffff_8000_0000_0000`) is a userspace RIP (the interrupted thread was in
/// user code); a high one is kernel code. Symbolize with addr2line.
pub fn current_cpu_tick_rip() -> u64 {
    let cpu = crate::cpu::cpu_id() as usize;
    if cpu < MAX_CORE_NUM {
        TICK_LAST_RIP_PERCPU[cpu].load(Relaxed)
    } else {
        0
    }
}

/// [diag] The RIP a timer tick last observed interrupting `cpu` (0 if never, or
/// `cpu` out of range). For a core wedged in an interrupts-off spin this stays
/// frozen where it entered the spin — so the deadlock banner can name where a
/// non-acking shootdown target actually is. Symbolize with addr2line.
pub fn cpu_tick_rip(cpu: usize) -> u64 {
    if cpu < MAX_CORE_NUM {
        TICK_LAST_RIP_PERCPU[cpu].load(Relaxed)
    } else {
        0
    }
}

/// [diag] Print every CPU's last-tick RIP with the IRQ-safe spin serial
/// writer. Used by corruption reports ([spine-smash]) to name what each core
/// was doing at most one tick before the detection — the writer is on one of
/// them. Symbolize with addr2line.
pub fn dump_last_tick_rips() {
    for cpu in 0..MAX_CORE_NUM {
        let rip = TICK_LAST_RIP_PERCPU[cpu].load(Relaxed);
        if rip != 0 {
            crate::console::serial_write_fmt_spin(format_args!(
                "[tick-rips]   cpu{cpu} last_tick_rip={rip:#x}\n"
            ));
        }
    }
}

/// [diag] Per-CPU RIP captured by the NMI handler. An NMI is delivered even to a
/// core spinning with interrupts disabled, so broadcasting one and reading these
/// slots gives the *current* instruction pointer of an otherwise-wedged core —
/// unlike the last-tick RIP, which freezes one tick *before* the spin begins.
static NMI_RIP_PERCPU: [AtomicU64; MAX_CORE_NUM] = [const { AtomicU64::new(0) }; MAX_CORE_NUM];

/// [diag] Record the interrupted RIP from the NMI handler (current CPU).
pub fn note_nmi_rip(rip: u64) {
    let cpu = crate::cpu::cpu_id() as usize;
    if cpu < MAX_CORE_NUM {
        NMI_RIP_PERCPU[cpu].store(rip, Relaxed);
    }
}

/// [diag] Most-recent page-fault instruction pointer, stashed by the trap
/// handler (which has the trap frame) so the kernel page-fault handler --
/// which only receives vaddr+flags -- can name the exact faulting code in
/// its panic message. Single global: faults are handled to completion
/// (panic) before the next, so there's no cross-fault race that matters.
static FAULT_RIP: AtomicU64 = AtomicU64::new(0);
static FAULT_RBP: AtomicU64 = AtomicU64::new(0);
static FAULT_RSP: AtomicU64 = AtomicU64::new(0);

/// [diag] Record the RIP of the instruction that just page-faulted.
pub fn note_fault_rip(rip: u64) {
    FAULT_RIP.store(rip, Relaxed);
}

/// [diag] Record the frame/stack pointers at the faulting instruction so the
/// page-fault handler can walk the call chain (e.g. name the caller of a wild
/// `memset`). Stored alongside the RIP by the arch trap entry.
pub fn note_fault_regs(rip: u64, rbp: u64, rsp: u64) {
    FAULT_RIP.store(rip, Relaxed);
    FAULT_RBP.store(rbp, Relaxed);
    FAULT_RSP.store(rsp, Relaxed);
}

/// [diag] Read back the last page-fault RIP recorded by `note_fault_rip`.
pub fn last_fault_rip() -> u64 {
    FAULT_RIP.load(Relaxed)
}

/// [diag] Read back the frame pointer at the last page fault.
pub fn last_fault_rbp() -> u64 {
    FAULT_RBP.load(Relaxed)
}

/// [diag] Read back the stack pointer at the last page fault.
pub fn last_fault_rsp() -> u64 {
    FAULT_RSP.load(Relaxed)
}

/// [diag] Broadcast an NMI to all other CPUs and busy-wait briefly so their NMI
/// handlers record their current RIP via `note_nmi_rip`. Call immediately before
/// `nmi_rips()`. No-op off bare x86_64.
#[cfg(all(target_arch = "x86_64", target_os = "none"))]
pub fn capture_cpu_rips() {
    zcore_drivers::irq::x86::Apic::send_nmi_all_others();
    let start = crate::timer::timer_now();
    while crate::timer::timer_now() < start + core::time::Duration::from_millis(2) {
        core::hint::spin_loop();
    }
}

/// [diag] No-op stub for non-bare / non-x86_64 targets.
#[cfg(not(all(target_arch = "x86_64", target_os = "none")))]
pub fn capture_cpu_rips() {}

/// [diag] Read the per-CPU RIPs captured by the last NMI broadcast.
pub fn nmi_rips() -> Vec<(u16, u64)> {
    let mut v = Vec::new();
    for (cpu, slot) in NMI_RIP_PERCPU.iter().enumerate() {
        let rip = slot.load(Relaxed);
        if rip != 0 {
            v.push((cpu as u16, rip));
        }
    }
    v
}

/// Account one hardware interrupt on `vector`.
pub fn note_irq(vector: usize) {
    IRQ_TOTAL.fetch_add(1, Relaxed);
    if vector < NVEC {
        IRQ_COUNTS[vector].fetch_add(1, Relaxed);
    }
}

/// A consistent-enough snapshot of the counters for rendering.
pub struct KStats {
    /// Total wall-clock all CPUs spent halted (summed across CPUs).
    pub idle_ns: u64,
    /// Number of idle naps entered.
    pub idle_entries: u64,
    /// Timer ticks handled.
    pub timer_ticks: u64,
    /// LAPIC deadline re-arms (timer pulled in ahead of the scheduler tick).
    pub timer_rearms: u64,
    /// Total interrupts handled.
    pub irq_total: u64,
    /// Idle-callback invocations.
    pub idle_cb_total: u64,
    /// Idle-callback invocations that found deferred work (kept the CPU awake).
    pub idle_cb_busy: u64,
    /// `(vector, count)` for every vector that fired at least once.
    pub irqs: Vec<(u16, u64)>,
    /// [diag] Per-CPU `(nap_count, total_nap_ns)` for cores that napped at least
    /// once, indexed by dense logical CPU id.
    pub idle_percpu: Vec<(u16, u64, u64)>,
    /// [diag] xHCI HID polls issued from the timer tick.
    pub hid_poll_timer: u64,
    /// [diag] xHCI HID polls issued from I/O-wait loops.
    pub hid_poll_iowait: u64,
    /// [diag] Per-CPU `(total_ticks, user_ticks, last_rip)` for cores that took
    /// at least one tick, indexed by dense logical CPU id. `user/total` localises
    /// a pegged core's busy time to ring 3 (user thread) vs ring 0 (kernel);
    /// `last_rip` is frozen at the spin entry for a wedged (no-tick) core.
    pub tick_percpu: Vec<(u16, u64, u64, u64)>,
}

/// Read the current counters.
pub fn snapshot() -> KStats {
    let mut irqs = Vec::new();
    for (v, c) in IRQ_COUNTS.iter().enumerate() {
        let n = c.load(Relaxed);
        if n != 0 {
            irqs.push((v as u16, n));
        }
    }
    let mut idle_percpu = Vec::new();
    for cpu in 0..MAX_CORE_NUM {
        let n = IDLE_ENTRIES_PERCPU[cpu].load(Relaxed);
        if n != 0 {
            idle_percpu.push((cpu as u16, n, IDLE_NS_PERCPU[cpu].load(Relaxed)));
        }
    }
    let mut tick_percpu = Vec::new();
    for cpu in 0..MAX_CORE_NUM {
        let t = TICK_TOTAL_PERCPU[cpu].load(Relaxed);
        if t != 0 {
            tick_percpu.push((
                cpu as u16,
                t,
                TICK_USER_PERCPU[cpu].load(Relaxed),
                TICK_LAST_RIP_PERCPU[cpu].load(Relaxed),
            ));
        }
    }
    KStats {
        idle_ns: IDLE_NS.load(Relaxed),
        idle_entries: IDLE_ENTRIES.load(Relaxed),
        timer_ticks: TIMER_TICKS.load(Relaxed),
        timer_rearms: TIMER_REARMS.load(Relaxed),
        irq_total: IRQ_TOTAL.load(Relaxed),
        idle_cb_total: IDLE_CB_TOTAL.load(Relaxed),
        idle_cb_busy: IDLE_CB_BUSY.load(Relaxed),
        irqs,
        idle_percpu,
        hid_poll_timer: HID_POLL_TIMER.load(Relaxed),
        hid_poll_iowait: HID_POLL_IOWAIT.load(Relaxed),
        tick_percpu,
    }
}

#[cfg(test)]
mod tests {
    //! Host tests for the CPU runtime-statistics counters. On libos
    //! `cpu::cpu_id()` is always 0, so every per-CPU update lands in slot 0.
    //!
    //! All counters here are process-global monotonic atomics and the test
    //! runner executes tests in parallel, so the assertions are written to be
    //! interference-proof: monotonic counters use `>=` deltas, per-vector IRQ
    //! checks each pick a vector no other test touches (exact delta), and the
    //! few tests that read non-monotonic shared state (the per-CPU idle flag,
    //! the last-RIP slot) serialise through `SERIAL`.
    use super::*;
    use spin::Mutex;

    static SERIAL: Mutex<()> = Mutex::new(());

    fn irq_count(snap: &KStats, vector: u16) -> u64 {
        snap.irqs
            .iter()
            .find(|(v, _)| *v == vector)
            .map(|(_, c)| *c)
            .unwrap_or(0)
    }

    #[test]
    fn note_irq_counts_specific_vector() {
        // Vector 0xF1 is used by no other test, so the delta is exactly ours.
        const V: u16 = 0xF1;
        let before = irq_count(&snapshot(), V);
        let total_before = snapshot().irq_total;
        for _ in 0..5 {
            note_irq(V as usize);
        }
        assert_eq!(irq_count(&snapshot(), V) - before, 5);
        assert!(snapshot().irq_total >= total_before + 5);
    }

    #[test]
    fn note_irq_out_of_range_still_counts_total() {
        // A vector beyond the tracked table bumps the grand total but no slot.
        let total_before = snapshot().irq_total;
        note_irq(NVEC + 10);
        assert!(snapshot().irq_total >= total_before + 1);
        // It must not have created a per-vector entry.
        assert!(snapshot().irqs.iter().all(|(v, _)| (*v as usize) < NVEC));
    }

    #[test]
    fn idle_accounting_is_monotonic() {
        let before = snapshot();
        for _ in 0..4 {
            note_idle(1000);
        }
        let after = snapshot();
        assert!(after.idle_ns >= before.idle_ns + 4000);
        assert!(after.idle_entries >= before.idle_entries + 4);
        // The per-CPU breakdown for cpu 0 must have grown too.
        let entries0 = |s: &KStats| {
            s.idle_percpu
                .iter()
                .find(|(c, _, _)| *c == 0)
                .map(|(_, n, _)| *n)
                .unwrap_or(0)
        };
        assert!(entries0(&after) >= entries0(&before) + 4);
    }

    #[test]
    fn timer_ticks_monotonic() {
        let before = snapshot().timer_ticks;
        for _ in 0..7 {
            note_timer_tick();
        }
        assert!(snapshot().timer_ticks >= before + 7);
    }

    #[test]
    fn hid_poll_counters_monotonic() {
        let before = snapshot();
        note_hid_poll_timer();
        note_hid_poll_timer();
        note_hid_poll_iowait();
        let after = snapshot();
        assert!(after.hid_poll_timer >= before.hid_poll_timer + 2);
        assert!(after.hid_poll_iowait >= before.hid_poll_iowait + 1);
    }

    #[test]
    fn idle_callback_busy_never_exceeds_total() {
        let before = snapshot();
        note_idle_callback(true); // found work
        note_idle_callback(false); // went to sleep
        note_idle_callback(false);
        let after = snapshot();
        assert!(after.idle_cb_total >= before.idle_cb_total + 3);
        assert!(after.idle_cb_busy >= before.idle_cb_busy + 1);
        // Structural invariant: busy is a subset of total, always.
        assert!(after.idle_cb_busy <= after.idle_cb_total);
    }

    #[test]
    fn snapshot_structural_invariants() {
        // Make sure there is some data to inspect.
        note_irq(0x42);
        note_idle(500);
        let s = snapshot();
        // Every reported vector fired at least once.
        assert!(s.irqs.iter().all(|(_, c)| *c > 0));
        // The grand total is at least the sum of the per-vector counts (the
        // total also includes out-of-range vectors).
        let per_vec_sum: u64 = s.irqs.iter().map(|(_, c)| *c).sum();
        assert!(s.irq_total >= per_vec_sum);
        // Idle naps imply idle time and vice-versa are both accounted.
        assert!(s.idle_entries > 0 && s.idle_ns > 0);
        // Per-CPU idle entries never exceed the global count.
        let percpu_entries: u64 = s.idle_percpu.iter().map(|(_, n, _)| *n).sum();
        assert!(percpu_entries <= s.idle_entries);
    }

    #[test]
    fn cpu_time_jiffies_reflect_noted_ns() {
        // CPU 9 is touched by no other test in this file, so the delta below
        // is exact rather than merely a lower bound.
        const CPU: usize = 9;
        const NS_PER_JIFFY: u64 = 10_000_000;
        let (u0, s0, _) = cpu_times_jiffies(CPU);
        note_user_time(CPU, 3 * NS_PER_JIFFY);
        note_sys_time(CPU, 5 * NS_PER_JIFFY);
        let (u1, s1, _) = cpu_times_jiffies(CPU);
        assert_eq!(u1 - u0, 3);
        assert_eq!(s1 - s0, 5);
    }

    #[test]
    fn cpu_time_jiffies_out_of_range_is_zero() {
        assert_eq!(cpu_times_jiffies(MAX_CORE_NUM + 1), (0, 0, 0));
        // Out-of-range notes must not panic (checked, not indexed blindly).
        note_user_time(MAX_CORE_NUM + 1, 1);
        note_sys_time(MAX_CORE_NUM + 1, 1);
    }

    #[test]
    fn cpu_idle_flag_roundtrip() {
        let _g = SERIAL.lock();
        // Only cpu 0 is ever marked on libos, and SERIAL keeps the other
        // flag-touching test out, so the count is exact here.
        set_cpu_idle(false);
        assert_eq!(cpus_idle_now(), 0);
        set_cpu_idle(true);
        assert_eq!(cpus_idle_now(), 1);
        set_cpu_idle(false);
        assert_eq!(cpus_idle_now(), 0);
    }

    #[test]
    fn tick_context_records_user_and_rip() {
        let _g = SERIAL.lock();
        let total0 = |s: &KStats| {
            s.tick_percpu
                .iter()
                .find(|(c, ..)| *c == 0)
                .map(|(_, t, ..)| *t)
                .unwrap_or(0)
        };
        let user0 = |s: &KStats| {
            s.tick_percpu
                .iter()
                .find(|(c, ..)| *c == 0)
                .map(|(_, _, u, _)| *u)
                .unwrap_or(0)
        };
        let before = snapshot();
        note_tick_context(true, 0xdead_beef); // interrupted user mode
        note_tick_context(false, 0xc0ff_ee00); // interrupted kernel mode
        let after = snapshot();
        // Two more ticks on cpu 0, exactly one of them in user mode.
        assert!(total0(&after) >= total0(&before) + 2);
        assert!(user0(&after) >= user0(&before) + 1);
        // user ticks are a subset of total ticks.
        let entry = after.tick_percpu.iter().find(|(c, ..)| *c == 0).unwrap();
        assert!(entry.2 <= entry.1);
        // The last recorded RIP is the most recent call's (kernel-mode one).
        assert_eq!(entry.3, 0xc0ff_ee00);
    }
}
