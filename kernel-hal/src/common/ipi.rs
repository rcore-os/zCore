use crate::{config::MAX_CORE_NUM, utils::mpsc_queue::MpscQueue};
use alloc::vec::Vec;

const REASON_SIZE: usize = 16;

pub type IpiEntry = usize;
type IRQueue = MpscQueue<'static, IpiEntry>;

/// Per-CPU backing storage for the IPI queues, indexed by dense logical CPU id.
static mut IPI_BUFFERS: [[IpiEntry; REASON_SIZE]; MAX_CORE_NUM] = [[0; REASON_SIZE]; MAX_CORE_NUM];

lazy_static::lazy_static! {
    /// One IPI queue per CPU, each backed by its slot in `IPI_BUFFERS`.
    static ref IPI_QUEUE: Vec<IRQueue> = (0..MAX_CORE_NUM)
        .map(|i| {
            IRQueue::new(unsafe {
                core::slice::from_raw_parts_mut(
                    core::ptr::addr_of_mut!(IPI_BUFFERS[i]).cast::<IpiEntry>(),
                    REASON_SIZE,
                )
            })
        })
        .collect();
}

pub(crate) fn ipi_queue(cpuid: usize) -> &'static IRQueue {
    &IPI_QUEUE[cpuid]
}

pub(crate) fn ipi_reason() -> Vec<usize> {
    let cpu_id = crate::cpu::cpu_id() as usize;
    let queue = ipi_queue(cpu_id);
    queue.consume_entrys().iter().map(|entry| entry.1).collect()
}

use core::sync::atomic::{AtomicU64, Ordering};

/// Bitmask of logical CPU ids that are actually online and able to service
/// IPIs. The BSP (logical 0) is always online; APs OR in their bit once they
/// reach `secondary_init`. Shootdowns only target online CPUs — APs that failed
/// to start (partial SMP bring-up) must not be signalled.
static CPU_ONLINE: AtomicU64 = AtomicU64::new(1);

/// Mark a logical CPU id as online (called from each CPU's bring-up path).
pub fn mark_cpu_online(logical_id: usize) {
    if logical_id < 64 {
        CPU_ONLINE.fetch_or(1u64 << logical_id, Ordering::Release);
    }
}

/// Number of logical CPUs that actually came online (BSP + every AP that
/// reached `secondary_init`). May be less than the detected/configured CPU
/// count when SMP bring-up is partial — useful for accounting that must not
/// divide by cores that never ran (e.g. the `/proc/perf` busy% denominator,
/// which would otherwise count a never-started AP as 100% busy).
pub fn online_cpu_count() -> usize {
    CPU_ONLINE.load(Ordering::Acquire).count_ones() as usize
}

/// Bitmask of CPUs that are actually *servicing* IPIs: running the executor
/// loop with interrupts enabled, so a TLB-shootdown IPI to them will be taken
/// and acknowledged promptly.
///
/// This is deliberately narrower than [`CPU_ONLINE`]. An AP is marked online in
/// `secondary_init` but then spins on the boot `STARTED` flag with interrupts
/// DISABLED until the BSP has spawned init — during which it cannot ack. Waiting
/// on such a CPU would stall *every* shootdown the BSP issues while it spawns
/// init (the heavy fork/exec/unmap burst) until the spin budget runs out, which
/// looks like a hang. A not-yet-ready CPU runs no user process, so it holds no
/// user TLB entry worth flushing — skipping it is safe.
static IPI_READY: AtomicU64 = AtomicU64::new(0);

/// Mark this CPU as ready to service TLB-shootdown IPIs. Called once, when the
/// CPU enters its executor loop with interrupts enabled.
pub fn mark_cpu_ipi_ready(logical_id: usize) {
    if logical_id < 64 {
        IPI_READY.fetch_or(1u64 << logical_id, Ordering::Release);
    }
}

/// Per-CPU TLB-shootdown acknowledgement counter. Each CPU bumps its own slot
/// every time it services a shootdown (i.e. flushes its whole TLB). A shootdown
/// initiator snapshots a target's counter before signalling it and then waits
/// for the counter to advance, which proves the target flushed *after* the
/// unmap — closing the stale-TLB window before the freed frame can be reused.
#[allow(clippy::declare_interior_mutable_const)]
const ZERO_SEQ: AtomicU64 = AtomicU64::new(0);
static SHOOTDOWN_SEQ: [AtomicU64; MAX_CORE_NUM] = [ZERO_SEQ; MAX_CORE_NUM];

/// Receiver side of the TLB shootdown: flush this CPU's whole TLB. The queue
/// payload is irrelevant — a full flush covers every pending request — so it is
/// just drained and discarded; this also makes the path robust to IPI-queue
/// overflow.
pub fn tlb_shootdown_ack() {
    let me = crate::cpu::cpu_id() as usize;
    crate::vm::flush_tlb(None);
    let _ = ipi_queue(me).discard_entrys();
    // Publish the completed flush LAST (Release) so an initiator that observes
    // the bump is guaranteed our TLB is already clean.
    if me < MAX_CORE_NUM {
        SHOOTDOWN_SEQ[me].fetch_add(1, Ordering::Release);
    }
}

/// Cross-CPU TLB shootdown.
///
/// x86 `flush_tlb` only invalidates the *local* CPU's TLB. Without this, after
/// one CPU unmaps/reprotects a page (COW copy-break, munmap, address-space
/// teardown) the other CPUs keep stale TLB entries pointing at the now-freed
/// physical frame; once it is reallocated to another VMO/process those entries
/// read/write the wrong owner's memory — the cross-process and kernel↔user
/// corruption that only shows up under SMP load.
///
/// Synchronous, but deadlock-proof. The initiator waits for every signalled CPU
/// to acknowledge the flush (so the freed frame cannot be reused while a stale
/// entry still points at it), with two safety valves so it can never hang:
///
///  * **Self-pump.** While waiting we service our OWN pending shootdowns, so two
///    CPUs that signal each other at the same instant cannot deadlock waiting on
///    each other's ack.
///  * **Bounded wait.** A target wedged with IRQs disabled (e.g. spinning on a
///    spinlock we currently hold) can't ack; after a spin budget we give up on
///    it and fall back to the old fire-and-forget behaviour for that CPU rather
///    than hang. That CPU still flushes when it next takes the IPI / context
///    switches, so this only narrows correctness in the rare contended window.
///
/// `vaddr` is advisory (each ack is a full flush, which covers every request).
pub fn remote_flush_tlb(_vaddr: Option<usize>) {
    let me = crate::cpu::cpu_id() as usize;
    // Only target CPUs that are actually servicing IPIs — NOT merely online.
    // Waiting on a CPU still spinning for `STARTED` with IRQs off (so it can't
    // ack) would stall the whole init spawn until the budget runs out.
    let targets = IPI_READY.load(Ordering::Acquire) & !(1u64 << me);
    if targets == 0 {
        return; // nobody else is servicing IPIs yet
    }
    let reason: IpiEntry = IpiReason::TlbShutdown { vpn: 0 }.into();
    // Snapshot each target's ack counter BEFORE signalling it, then signal.
    let mut snapshot = [0u64; MAX_CORE_NUM];
    for cpu in 0..MAX_CORE_NUM {
        if targets & (1u64 << cpu) != 0 {
            snapshot[cpu] = SHOOTDOWN_SEQ[cpu].load(Ordering::Acquire);
            let _ = crate::interrupt::send_ipi(cpu, reason);
        }
    }
    // Total spin budget across all targets: generous enough that a real ack from
    // an IRQ-on target (microseconds) always arrives first, bounded so a target
    // briefly wedged with IRQs off (spinning on a lock we hold) is abandoned in
    // finite time — degrading to fire-and-forget for it — instead of hanging.
    const SPIN_BUDGET: u64 = 1 << 15;
    let mut spins: u64 = 0;
    loop {
        // A target that is *halted* (idle) needs no synchronous wait: it is not
        // executing, so it has no live use of the freed frame, and the shootdown
        // IPI already queued to it flushes its TLB when it wakes — before it runs
        // any user instruction — exactly as the budget-exhaustion fire-and-forget
        // fallback below already relies on. Treating idle targets as satisfied
        // reaches that same safe state immediately instead of burning the whole
        // budget on a halted core that is slow to ack (the dominant cause of
        // fork/exec stalls on real multi-core hardware). Re-read each spin so a
        // target that parks mid-wait is dropped too; a target still *running* is
        // waited on as before.
        let idle = crate::kstats::cpu_idle_mask();
        let mut all_acked = true;
        for cpu in 0..MAX_CORE_NUM {
            if targets & (1u64 << cpu) != 0
                && idle & (1u64 << cpu) == 0
                && SHOOTDOWN_SEQ[cpu].load(Ordering::Acquire) == snapshot[cpu]
            {
                all_acked = false;
            }
        }
        if all_acked {
            break;
        }
        // Self-pump: if a peer asked US to flush, do it now (non-allocating) so
        // it isn't blocked on our ack while we block on its.
        let q = ipi_queue(me);
        if q.chead() < q.ptail() {
            tlb_shootdown_ack();
        }
        spins += 1;
        if spins >= SPIN_BUDGET {
            break; // bounded fallback to fire-and-forget, never a hang
        }
        core::hint::spin_loop();
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum IpiReason {
    Invalid,
    MockBlock { block_info: usize },
    TlbShutdown { vpn: usize }, // unused
}

// usize : 64bit
// |  type reason : 4bit  |   ipi info : 60bit   |
//
// MockBlock info : 60bit
// |  reserved : 60 bit  |
const TYPE_SHIFT: usize = 60;
const TYPE_INVALID: usize = 0x0;
const TYPE_MOCK_BLOCK: usize = 0x1;
const TYPE_TLB_SHUTDOWN: usize = 0x2;

impl From<IpiEntry> for IpiReason {
    fn from(r: IpiEntry) -> Self {
        let ipi_type = r >> TYPE_SHIFT;
        let ipi_info = r & 0x000FFFFFFFFFFFFF;
        match ipi_type {
            TYPE_MOCK_BLOCK => Self::MockBlock {
                block_info: ipi_info,
            },
            TYPE_TLB_SHUTDOWN => Self::TlbShutdown { vpn: ipi_info },
            _ => Self::Invalid,
        }
    }
}

impl From<IpiReason> for IpiEntry {
    fn from(reason: IpiReason) -> Self {
        match reason {
            IpiReason::MockBlock { block_info: info } => (TYPE_MOCK_BLOCK << TYPE_SHIFT) | info,
            IpiReason::TlbShutdown { vpn: info } => (TYPE_TLB_SHUTDOWN << TYPE_SHIFT) | info,
            IpiReason::Invalid => 0,
        }
    }
}
