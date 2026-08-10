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

/// Bitmask of CPUs that reached `secondary_init` (BSP bit 0 always set).
/// Prefer this over `(1 << cpu_count()) - 1`, which includes APs that never
/// came online.
pub fn cpu_online_mask() -> u64 {
    CPU_ONLINE.load(Ordering::Acquire)
}

/// Number of logical CPUs that actually came online (BSP + every AP that
/// reached `secondary_init`). May be less than the detected/configured CPU
/// count when SMP bring-up is partial — useful for accounting that must not
/// divide by cores that never ran (e.g. the `/proc/perf` busy% denominator,
/// which would otherwise count a never-started AP as 100% busy).
pub fn online_cpu_count() -> usize {
    cpu_online_mask().count_ones() as usize
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

/// Page-table root (CR3 frame) each CPU currently has loaded, or 0 = unknown.
///
/// Written by `activate_paging` BEFORE the hardware switch: a flusher that
/// reads the OLD token and skips the switching CPU is still correct, because
/// the CR3 write it is racing flushes every non-global entry anyway. Written
/// as the frame base (low 12 bits masked) so flag bits at the call sites
/// cannot break the comparison.
static ACTIVE_VMTOKEN: [core::sync::atomic::AtomicUsize; MAX_CORE_NUM] =
    [const { core::sync::atomic::AtomicUsize::new(0) }; MAX_CORE_NUM];

/// Record that this CPU is about to load `token` (a page-table root).
pub fn note_active_vmtoken(token: usize) {
    let me = crate::cpu::cpu_id() as usize;
    if me < MAX_CORE_NUM {
        ACTIVE_VMTOKEN[me].store(token & !0xfff, Ordering::SeqCst);
    }
}

/// Mark this CPU as ready to service TLB-shootdown IPIs. Called once, when the
/// CPU enters its executor loop with interrupts enabled.
pub fn mark_cpu_ipi_ready(logical_id: usize) {
    if logical_id < 64 {
        IPI_READY.fetch_or(1u64 << logical_id, Ordering::Release);
    }
}

/// Per-CPU TLB-shootdown acknowledgement counter. Each CPU bumps its own slot
/// every time it services a shootdown. A shootdown initiator snapshots a
/// target's counter before signalling it and then waits for the counter to
/// advance, which proves the target flushed *after* the unmap — closing the
/// stale-TLB window before the freed frame can be reused.
#[allow(clippy::declare_interior_mutable_const)]
const ZERO_SEQ: AtomicU64 = AtomicU64::new(0);
static SHOOTDOWN_SEQ: [AtomicU64; MAX_CORE_NUM] = [ZERO_SEQ; MAX_CORE_NUM];

/// Per-CPU "the IPI queue could not take an entry" flags. A sender that fails
/// to publish its payload (queue full / lost commit race) sets the target's
/// bit; the target's next ack then falls back to a full TLB flush, so the
/// precise per-page path below can never silently skip an invalidation.
static IPI_QUEUE_OVERFLOW: AtomicU64 = AtomicU64::new(0);

/// Note that `cpuid`'s IPI queue dropped an entry (called by the arch sender).
pub fn note_ipi_queue_overflow(cpuid: usize) {
    if cpuid < 64 {
        IPI_QUEUE_OVERFLOW.fetch_or(1u64 << cpuid, Ordering::Release);
    }
}

/// Max shootdown entries serviced precisely (per-page `invlpg`) in one ack;
/// beyond this a full flush is cheaper than the invlpg sequence.
const MAX_PRECISE_SHOOTDOWN: usize = 8;

/// Receiver side of the TLB shootdown.
///
/// Drains the queue and services the requests:
///  * empty drain and no overflow → **pure wake** (a reschedule kick): return
///    without touching the TLB and *without* bumping the ack sequence, so a
///    shootdown initiator can never mistake a wake for a completed flush.
///  * a few well-formed per-page requests → `invlpg` each (user PTEs are
///    never GLOBAL here, so a targeted invalidation fully covers the request
///    on x86; see `vm.rs`).
///  * anything else (overflow flag, full-flush sentinel `vpn == 0`, or too
///    many entries) → one full flush, the previous behaviour.
/// Spin-loop pump: drain this CPU's pending shootdown queue if — and only if —
/// there is something in it. One queue-pointer compare when idle, so it is
/// cheap enough to call every few hundred spins from inside a held-IRQs-off
/// spin loop (see kernel-sync's `set_spin_pump`).
pub fn tlb_shootdown_pump() {
    let me = crate::cpu::cpu_id() as usize;
    if me >= MAX_CORE_NUM || IPI_READY.load(Ordering::Relaxed) & (1u64 << me) == 0 {
        return;
    }
    let q = ipi_queue(me);
    if q.chead() == q.ptail() && IPI_QUEUE_OVERFLOW.load(Ordering::Relaxed) & (1u64 << me) == 0 {
        return;
    }
    tlb_shootdown_ack();
}

pub fn tlb_shootdown_ack() {
    let me = crate::cpu::cpu_id() as usize;
    if me >= MAX_CORE_NUM {
        return;
    }
    // Order: consume the overflow flag BEFORE draining. Any sender that set it
    // did so before ringing the IPI, so either we see the flag here, or the
    // flag-setter's interrupt is still pending and the NEXT ack handles it.
    let overflow =
        IPI_QUEUE_OVERFLOW.fetch_and(!(1u64 << me), Ordering::AcqRel) & (1u64 << me) != 0;
    // Non-allocating bounded drain of this CPU's queue (single consumer).
    let q = ipi_queue(me);
    let mut vpns = [0usize; MAX_PRECISE_SHOOTDOWN];
    let mut n_vpns = 0usize;
    let mut precise = true;
    // Only TlbShutdown / overflow may bump SHOOTDOWN_SEQ. MockBlock (and other
    // non-TLB reasons) used to demote to a full flush *and* bump seq, which
    // let a concurrent shootdown initiator observe a false ack and free frames
    // while our TLB still held the stale mapping.
    let mut saw_tlb = overflow;
    let chead = q.chead();
    let ptail = q.ptail();
    for idx in chead..ptail {
        let entry = *q.entry_at(idx);
        match IpiReason::from(entry) {
            IpiReason::TlbShutdown { vpn } if vpn != 0 => {
                saw_tlb = true;
                if n_vpns < MAX_PRECISE_SHOOTDOWN {
                    vpns[n_vpns] = vpn;
                    n_vpns += 1;
                } else {
                    precise = false;
                }
            }
            IpiReason::TlbShutdown { vpn: 0 } => {
                // Full-flush sentinel.
                saw_tlb = true;
                precise = false;
            }
            // Non-TLB payload (e.g. MockBlock): drain it so it cannot block
            // the queue, but do NOT treat it as a shootdown ack.
            _ => {}
        }
    }
    if chead == ptail && !overflow {
        // Pure wake IPI: nothing to flush, nothing to acknowledge.
        return;
    }
    // Consume exactly the snapshot we serviced — never past it. Entries that
    // commit after `ptail` was read stay queued for the ack their own IPI
    // triggers (the old `discard_entrys()` jumped to the *current* tail, which
    // could drop a just-committed request whose PTE change our flush above
    // predates). CAS so a nested ack (IRQ landing inside the initiator's
    // self-pump) that already advanced the head is never rewound.
    let _ = q
        .chead
        .compare_exchange(chead, ptail, Ordering::AcqRel, Ordering::Relaxed);
    if !saw_tlb {
        // Drained only non-TLB reasons — leave SHOOTDOWN_SEQ alone.
        return;
    }
    if precise && !overflow {
        for &vpn in &vpns[..n_vpns] {
            crate::vm::flush_tlb(Some(vpn << 12));
        }
    } else {
        crate::vm::flush_tlb(None);
    }
    // Publish the completed flush LAST (Release) so an initiator that observes
    // the bump is guaranteed our TLB is already clean.
    SHOOTDOWN_SEQ[me].fetch_add(1, Ordering::Release);
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
/// Synchronous. The initiator waits for every signalled CPU to acknowledge the
/// flush (so the freed frame cannot be reused while a stale entry still points
/// at it), with:
///
///  * **Self-pump.** While waiting we service our OWN pending shootdowns, so two
///    CPUs that signal each other at the same instant cannot deadlock waiting on
///    each other's ack.
///  * **Long bounded wait** with repeated self-pump. We do **not** treat idle
///    targets as acked (TOCTOU with wake→user) and we do **not** silently
///    fire-and-forget after a short budget — that was the COW/TLB corruption
///    class under SMP. Ticket-lock spin-pump covers the common IRQs-off case.
///
/// `vaddr`, when given, is delivered to each target so its ack can `invlpg`
/// just that page instead of flushing its whole TLB; `None` (or a dropped
/// queue entry) demotes the ack to a full flush.
pub fn remote_flush_tlb(vaddr: Option<usize>) {
    remote_flush_tlb_aspace(vaddr, None)
}

/// [`remote_flush_tlb`] with an optional address-space filter.
///
/// `aspace = Some(root)`: only CPUs whose ACTIVE page-table root is `root` (or
/// unknown) are targeted. Sound without PCID because a CR3 write flushes every
/// non-global entry: a CPU that switched away from this address space has no
/// stale user entries left to invalidate, and a CPU switching TO it publishes
/// its token before the CR3 write (see `note_active_vmtoken`). Kernel-table
/// flushes pass `None` and keep targeting everyone — global-bit entries
/// survive CR3 writes, so no CPU can be filtered out for those.
///
/// Under a fork/exec-heavy parallel load this is most of the win: unrelated
/// processes on other CPUs stop taking (and stop having to ack) IPIs for
/// address spaces they have never loaded.
pub fn remote_flush_tlb_aspace(vaddr: Option<usize>, aspace: Option<usize>) {
    let me = crate::cpu::cpu_id() as usize;
    // Only target CPUs that are actually servicing IPIs — NOT merely online.
    // Waiting on a CPU still spinning for `STARTED` with IRQs off (so it can't
    // ack) would stall the whole init spawn until the budget runs out.
    let mut targets = IPI_READY.load(Ordering::Acquire) & !(1u64 << me);
    if let Some(root) = aspace {
        let root = root & !0xfff;
        for cpu in 0..MAX_CORE_NUM {
            if targets & (1u64 << cpu) != 0 {
                let tok = ACTIVE_VMTOKEN[cpu].load(Ordering::SeqCst);
                if tok != 0 && tok != root {
                    targets &= !(1u64 << cpu);
                }
            }
        }
    }
    if targets == 0 {
        return; // nobody else is servicing IPIs yet, or nobody has this aspace
    }
    // vpn 0 doubles as the full-flush sentinel (page 0 is never mapped).
    let reason: IpiEntry = IpiReason::TlbShutdown {
        vpn: vaddr.map_or(0, |va| va >> 12),
    }
    .into();
    // Snapshot each target's ack counter BEFORE signalling it, then signal.
    let mut snapshot = [0u64; MAX_CORE_NUM];
    for cpu in 0..MAX_CORE_NUM {
        if targets & (1u64 << cpu) != 0 {
            snapshot[cpu] = SHOOTDOWN_SEQ[cpu].load(Ordering::Acquire);
            let _ = crate::interrupt::send_ipi(cpu, reason);
        }
    }
    // Wait until every target advances past its snapshot. Idle skip was removed:
    // a CPU can leave idle and run user code with a stale TLB before taking the
    // pending IPI (TOCTOU). Spin-pump on ticket locks covers IRQs-off holders.
    // Soft warn after a long wait; keep waiting (correctness > latency).
    const SPIN_WARN: u64 = 1 << 24;
    let mut spins: u64 = 0;
    let mut warned = false;
    loop {
        let mut all_acked = true;
        for cpu in 0..MAX_CORE_NUM {
            if targets & (1u64 << cpu) != 0
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
        if q.chead() < q.ptail()
            || IPI_QUEUE_OVERFLOW.load(Ordering::Relaxed) & (1u64 << me) != 0
        {
            tlb_shootdown_ack();
        }
        spins += 1;
        if spins >= SPIN_WARN && !warned {
            warned = true;
            crate::console::serial_write_fmt_spin(format_args!(
                "\n[tlb-shootdown] slow ack wait spins={} targets={:#x} me={}\n",
                spins, targets, me,
            ));
        }
        core::hint::spin_loop();
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum IpiReason {
    Invalid,
    MockBlock {
        block_info: usize,
    },
    /// TLB shootdown request. `vpn` = the page to invalidate (vaddr >> 12);
    /// 0 requests a full flush.
    TlbShutdown {
        vpn: usize,
    },
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
