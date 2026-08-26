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

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Master switch for application-processor (SMP) bring-up. Default **ON**;
/// `smp=off` on the kernel cmdline forces a single-core boot.
///
/// This defaulted off while multi-core wedged real hardware at the hand-off to
/// the scheduler (boot reached 100% and stopped). The cause was the APIC id
/// plumbing, not the scheduler: `LocalApicBuilder` switches the LAPIC into
/// x2APIC mode on any CPU that advertises support for it — every real x86 since
/// roughly 2008, and *not* QEMU's default TCG CPU, which is why only physical
/// machines were affected. In that mode the LAPIC stops decoding its MMIO page,
/// but `kernel-sync` still read the APIC id from that window, so every CPU
/// resolved to the same bogus id and the APs shared one per-CPU slot. Those
/// reads now go through the MSR interface (see `kernel-sync::interrupt` and
/// `drivers::irq::x86_apic::lapic`), so the ids are distinct again.
///
/// `smp=off` stays as the escape hatch for bringing a suspect machine up
/// single-core without a rebuild.
static SMP_ENABLED: AtomicBool = AtomicBool::new(true);

/// Override AP bring-up (called from `zCore` when `smp=off` is on the cmdline).
pub fn set_smp_enabled(v: bool) {
    SMP_ENABLED.store(v, Ordering::Relaxed);
}

/// Whether AP bring-up is enabled. Read by `start_application_processors`.
pub fn smp_enabled() -> bool {
    SMP_ENABLED.load(Ordering::Relaxed)
}

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

/// Per-CPU TLB-shootdown acknowledgement watermark: the queue index (the
/// drain's consumed `ptail`) this CPU has flushed up to. An initiator records
/// its target-queue `ptail` right after enqueueing its request and waits for
/// this watermark to REACH it — proof the target flushed after consuming that
/// very request, not merely that "some" drain completed (the plain counter
/// this used to be had exactly that TOCTOU; see the publish site in
/// `tlb_shootdown_ack`).
#[allow(clippy::declare_interior_mutable_const)]
const ZERO_SEQ: AtomicU64 = AtomicU64::new(0);
static SHOOTDOWN_SEQ: [AtomicU64; MAX_CORE_NUM] = [ZERO_SEQ; MAX_CORE_NUM];

/// Per-CPU "an ack is in flight on this CPU right now" flag, set for the
/// duration of [`tlb_shootdown_ack`]. Read ONLY by the NMI-driven ack
/// ([`tlb_shootdown_ack_nmi`]): if a normal pump/IRQ ack is already draining
/// this CPU's queue, the NMI must NOT re-enter it (that would double-drain),
/// so it skips and lets the in-flight ack finish. When the flag is clear the
/// CPU is wedged somewhere *other* than an ack (a fault storm, an IRQs-off
/// busy-wait) and the NMI is the only thing that can service the queue for it.
#[allow(clippy::declare_interior_mutable_const)]
const FALSE_FLAG: AtomicBool = AtomicBool::new(false);
static SHOOTDOWN_ACK_ACTIVE: [AtomicBool; MAX_CORE_NUM] = [FALSE_FLAG; MAX_CORE_NUM];

/// [diag] Per-CPU "I am spin-waiting for these CPUs to ack my shootdown" mask,
/// published live by [`remote_flush_tlb_aspace`]'s wait loop (0 = not waiting).
/// The deadlock banner reads it: a lock HOLDER that is *also* here is the
/// convoy's head — it is stuck because the CPUs in its mask never acked (a
/// non-pumping IRQs-off spinner, e.g. deep in vendor RM MMIO polling), NOT
/// because of a lock-ordering cycle. That single bitmask is what tells an
/// on-screen-only (no serial) hardware capture "shootdown starvation" apart
/// from "AB-BA", and names the CPU to go look at.
static SHOOTDOWN_WAIT_MASK: [AtomicU64; MAX_CORE_NUM] = [ZERO_SEQ; MAX_CORE_NUM];

/// The set of CPUs `cpu` is currently blocked waiting on for a TLB-shootdown
/// ack, or 0 if it is not in a shootdown wait. Racy by nature — diagnostics.
pub fn shootdown_wait_mask(cpu: usize) -> u64 {
    if cpu < MAX_CORE_NUM {
        SHOOTDOWN_WAIT_MASK[cpu].load(Ordering::Relaxed)
    } else {
        0
    }
}

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
    // Publish that a drain is in flight on this CPU, and clear it on every exit
    // (the guard's Drop), so an NMI-driven ack landing mid-drain skips instead
    // of double-consuming the queue. Set AFTER the range check so `me` is valid.
    SHOOTDOWN_ACK_ACTIVE[me].store(true, Ordering::SeqCst);
    let _ack_active = AckActiveGuard(me);
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
    // it is guaranteed our TLB is already clean.
    //
    // The published value is the queue index this drain CONSUMED UP TO — not a
    // plain +1 counter. A "+1 means acked" protocol had a real TOCTOU, caught
    // live by the fork-hammer's torn-page failures: a drain already in flight
    // when the initiator's entry was enqueued (its `ptail` snapshot taken just
    // before the commit) finishes, flushes only the OLDER pages, and bumps the
    // counter — the initiator mistakes that for its own ack and returns while
    // its entry is still queued and the target's TLB still holds the stale
    // writable entry for MICROSECONDS more. A hot writer thread on that CPU
    // keeps storing into a frame a fork child now shares — the 3-generation
    // torn pages. Publishing the consumed index makes the ack unambiguous:
    // the initiator waits for `SHOOTDOWN_SEQ >= the ptail it observed right
    // after its own enqueue`, which no earlier drain can satisfy. fetch_max
    // because the pump/IRQ/NMI paths race benignly (drains are serialized by
    // SHOOTDOWN_ACK_ACTIVE, but keep the publish monotone regardless).
    SHOOTDOWN_SEQ[me].fetch_max(ptail as u64, Ordering::Release);
}

/// Clears [`SHOOTDOWN_ACK_ACTIVE`] on scope exit, covering every early return in
/// [`tlb_shootdown_ack`].
struct AckActiveGuard(usize);
impl Drop for AckActiveGuard {
    fn drop(&mut self) {
        SHOOTDOWN_ACK_ACTIVE[self.0].store(false, Ordering::SeqCst);
    }
}

/// Service this CPU's pending shootdown from the NMI handler.
///
/// A normal 0xf3 IPI reaches a CPU only when it takes interrupts. A CPU wedged
/// with IRQs off — deep in a fault storm, a non-pumping busy-wait, or corrupt
/// code — never does, so it starves any peer waiting on its ack, and that wait
/// has no timeout (correctness > latency). An NMI is delivered regardless; this
/// is what it runs, the same non-allocating, lock-free drain as the pump path.
///
/// NMI-safe: it takes no locks, allocates nothing, and prints nothing (a print
/// would deadlock against a console lock the interrupted code may hold). It
/// skips when a normal ack is already draining this CPU's queue (the guard
/// flag) so it never double-consumes the single-consumer queue, and no-ops when
/// nothing is pending. NMIs do not nest, so the flag check is race-free here.
pub fn tlb_shootdown_ack_nmi() {
    let me = crate::cpu::cpu_id() as usize;
    if me >= MAX_CORE_NUM {
        return;
    }
    // A drain is already in flight here (the NMI interrupted a pump/IRQ ack) —
    // let it finish rather than re-enter and double-consume the queue.
    if SHOOTDOWN_ACK_ACTIVE[me].load(Ordering::SeqCst) {
        return;
    }
    // Nothing queued and no overflow: not a CPU anyone is starving on.
    let q = ipi_queue(me);
    if q.chead() == q.ptail() && IPI_QUEUE_OVERFLOW.load(Ordering::Relaxed) & (1u64 << me) == 0 {
        return;
    }
    tlb_shootdown_ack();
}

/// Broadcast an NMI to every other CPU so a wedged target services its pending
/// shootdown ([`tlb_shootdown_ack_nmi`]). x86_64/bare only; a no-op elsewhere.
/// Healthy CPUs no-op the ack, so the broadcast only actually helps the stuck
/// one; a targeted NMI would need a new low-level APIC entry point and buys
/// nothing here, where this runs only once a shootdown is already starving.
#[cfg(all(target_arch = "x86_64", target_os = "none"))]
fn nmi_kick_pending_targets() {
    zcore_drivers::irq::x86::Apic::send_nmi_all_others();
}
#[cfg(not(all(target_arch = "x86_64", target_os = "none")))]
fn nmi_kick_pending_targets() {}

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
    // Single-core short-circuit — and a corrupt-`cpu_id` safety net.
    //
    // On a uniprocessor boot there is no other CPU that can hold a stale TLB
    // entry, so a cross-CPU shootdown is *by definition* a local flush and must
    // never wait on anyone. Skipping the machinery entirely here also closes a
    // wedge seen in the field: when memory corruption clobbers this CPU's
    // per-CPU identity, `cpu_id()` returns a bogus non-zero id (e.g. 2 on a
    // single-core box). `targets = IPI_READY & !(1<<me)` then keeps bit 0 set —
    // a *phantom* target that is really us — and the loop below spins forever
    // (`spins=16777216 targets=0x1 me=2`) self-pumping the wrong queue, so the
    // machine hangs right after a fault was otherwise cleanly contained. When
    // only one CPU is online, no stale-TLB correctness is at stake, so a local
    // flush is both sufficient and the only safe thing to do.
    if online_cpu_count() <= 1 {
        crate::vm::flush_tlb(vaddr);
        return;
    }
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
    // Signal each target, then record the GOAL its ack must reach: the
    // target-queue `ptail` observed right after our enqueue. `send_ipi`
    // commits the entry (or sets the overflow bit) BEFORE ringing the APIC
    // and before returning, so this ptail is `>= our entry's index + 1` — and
    // a drain that publishes a consumed-index `>= goal` has provably flushed
    // AFTER consuming our request (or full-flushed on the overflow bit, whose
    // consuming drain also reaches this goal). Waiting on a bare "counter
    // advanced" was a TOCTOU: an in-flight drain that predated our enqueue
    // bumped it without servicing us. See the publish site in
    // `tlb_shootdown_ack`.
    //
    // A CPU whose IPI could not be delivered is dropped from the wait set: it
    // will never acknowledge, and the loop below has no timeout, so keeping it
    // as a target is an unconditional hang. That is strictly worse than the
    // stale mapping it reports — and the drop is loud, because a shootdown we
    // could not deliver does leave that CPU's TLB unflushed.
    let mut goal = [0u64; MAX_CORE_NUM];
    for cpu in 0..MAX_CORE_NUM {
        if targets & (1u64 << cpu) != 0 {
            if crate::interrupt::send_ipi(cpu, reason).is_err() {
                targets &= !(1u64 << cpu);
                crate::console::serial_write_fmt_spin(format_args!(
                    "\n[tlb-shootdown] cpu {} unreachable — skipped (its TLB may be stale)\n",
                    cpu,
                ));
            } else {
                goal[cpu] = ipi_queue(cpu).ptail() as u64;
            }
        }
    }
    if targets == 0 {
        return;
    }
    // Wait until every target's flush watermark reaches its goal. Idle skip was removed:
    // a CPU can leave idle and run user code with a stale TLB before taking the
    // pending IPI (TOCTOU). Spin-pump on ticket locks covers IRQs-off holders.
    // Soft warn after a long wait; keep waiting (correctness > latency).
    const SPIN_WARN: u64 = 1 << 24;
    let mut spins: u64 = 0;
    let mut warned = false;
    loop {
        let mut all_acked = true;
        let mut pending = 0u64;
        for cpu in 0..MAX_CORE_NUM {
            if targets & (1u64 << cpu) != 0
                && SHOOTDOWN_SEQ[cpu].load(Ordering::Acquire) < goal[cpu]
            {
                all_acked = false;
                pending |= 1u64 << cpu;
            }
        }
        // [diag] Publish who we are still blocked on, so the deadlock banner can
        // show — with no serial, on a real-hardware screen capture — that this
        // CPU is a shootdown-starvation victim and name the CPU(s) not acking.
        SHOOTDOWN_WAIT_MASK[me].store(pending, Ordering::Relaxed);
        if all_acked {
            SHOOTDOWN_WAIT_MASK[me].store(0, Ordering::Relaxed);
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
        // Re-deliver the shootdown to still-pending targets periodically. A
        // target that took its IPI as a pure wake BEFORE our queue entry became
        // visible (the enqueue/signal TOCTOU handled by tlb_shootdown_ack) never
        // bumped its ack — and if it is alive with IRQs on but not spinning on
        // any ticket lock, it never pumps and never gets another IPI, so it
        // would starve this shootdown forever. Re-sending makes that lost wakeup
        // self-heal. It is harmless when the original entry is still queued (the
        // target just flushes the page twice) and correctness-safe on a full
        // queue (the overflow bit demotes the target's next ack to a full
        // flush). Gated far past the healthy fast path — which acks within a
        // handful of spins — so the common case never re-kicks, and only the
        // CPUs still in `pending` this iteration are poked.
        if spins & ((1 << 16) - 1) == 0 {
            for cpu in 0..MAX_CORE_NUM {
                if pending & (1u64 << cpu) != 0 {
                    let _ = crate::interrupt::send_ipi(cpu, reason);
                }
            }
        }
        // Escalate to NMI when the IPI re-kick has not helped for a while: the
        // target is not merely missing a wakeup but wedged where no maskable
        // interrupt lands (IRQs off, a fault storm, corrupt code). An NMI is
        // delivered regardless and its handler drains+acks the target's queue
        // (tlb_shootdown_ack_nmi). 16x rarer than the re-kick and far below the
        // deadlock detector's window, so a genuine lost wakeup is still handled
        // by the cheaper targeted IPI first; this is the last-resort unwedge.
        if spins & ((1 << 20) - 1) == 0 {
            nmi_kick_pending_targets();
        }
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
