//! Unmapped guard bands around the scheduler's coroutine stacks.
//!
//! A coroutine stack is a plain heap allocation (`PreemptiveScheduler`'s
//! `Executor::new`), laid out as
//! `[bottom guard][usable STACK_SIZE][top guard]`, and the kernel heap is a
//! 512 MiB `static mut` array in `.bss`. So a stack that runs off either end
//! does not fault — it quietly overwrites whichever heap object happens to sit
//! next to it. That silent overwrite is the root of the whole corruption hunt
//! in `docs/README-crash-repro.md`: return addresses with a mangled top byte,
//! `Arc` vtables replaced by `0x87`, a `memset` handed a pointer with one byte
//! flipped, and indirect calls landing on `rip=0x0`/`0x3`.
//!
//! This module makes those two bands **unmapped**, so the overflow takes a
//! clean page fault that `zcore::handler` reports as
//! `[stack-guard] COROUTINE STACK OVERFLOW` — naming the bug instead of
//! scattering it across unrelated memory. The scheduler falls back to filling
//! the bands with canary words if [`install`] refuses, which still *detects*
//! an overflow (at the next poll or timer tick) but only after the heap is
//! already corrupt.
//!
//! # How a page is taken away
//!
//! Not by unmapping it: by **clearing every permission bit while leaving the
//! physical address in the entry**. On all three architectures an empty
//! `MMUFlags` converts to an entry with no present/valid bit, so the CPU faults
//! on any access, while `PageTableEntry::addr()` still holds the frame. That
//! matters because these frames are `.bss` — they belong to the kernel image,
//! not to the frame allocator — and this way the page table itself remembers
//! them: there is no side table of physical addresses to keep in sync, nothing
//! to allocate (so no heap re-entry from inside `Executor::new`), and no way
//! for a bookkeeping slip to hand a `.bss` frame to the frame allocator.
//! [`remove`] just writes the original flags back.
//!
//! Everything here is all-or-nothing. Any surprise — a huge PTE covering the
//! band, a page that is not mapped, flags that differ across the band, a
//! verification read that does not show the page gone — rolls the whole band
//! back and returns `false`, which is exactly the soft-canary fallback the
//! scheduler already handles. The failure mode of this module is "no better
//! than before", never "worse".

use core::sync::atomic::{AtomicUsize, Ordering};

use crate::vm::{GenericPageTable, PageTable};
use crate::MMUFlags;

const PAGE_SIZE: usize = 4096;

/// Live guard bands. Two per executor (bottom and top); the scheduler's own
/// history shows a few dozen executors alive at once during a desktop session,
/// so this is sized well past that. Exhausting it is not fatal — [`install`]
/// refuses and that executor falls back to the soft canary.
const MAX_GUARDS: usize = 1024;

/// Registry of installed bands: `base == 0` marks a free slot.
///
/// A plain lock-free table rather than a `Mutex<Vec<_>>` because
/// [`is_guard_fault`] is called from the page-fault handler, where taking a
/// lock — or allocating — risks turning a diagnosable fault into a deadlock.
static GUARD_BASE: [AtomicUsize; MAX_GUARDS] = [const { AtomicUsize::new(0) }; MAX_GUARDS];
static GUARD_END: [AtomicUsize; MAX_GUARDS] = [const { AtomicUsize::new(0) }; MAX_GUARDS];
/// Original flags of the band's pages, to be written back by [`remove`].
static GUARD_FLAGS: [AtomicUsize; MAX_GUARDS] = [const { AtomicUsize::new(0) }; MAX_GUARDS];

/// One past the highest slot index ever claimed, so [`slot_of`] does not walk
/// the whole table on every kernel page fault. Slots are reused, so this
/// settles at roughly twice the number of executors alive at once.
static GUARD_HIGH: AtomicUsize = AtomicUsize::new(0);

/// Bands installed and removed since boot, for the boot log.
static INSTALLED: AtomicUsize = AtomicUsize::new(0);
static REFUSED: AtomicUsize = AtomicUsize::new(0);

/// Register this module's hooks with the scheduler.
///
/// Must run after the kernel page tables are pinned and *before* the first
/// `Executor::new`, which is why `primary_init` calls it immediately before
/// `executor::warm_runtimes()`. Boot asserts that the hooks got registered.
pub fn init() {
    // SAFETY: the contract is that `remove` restores any mapping `install`
    // took away before the VA goes back to the heap, and that no `.bss`-owned
    // frame is handed to the frame allocator. `install` never takes a frame out
    // of its page table entry, so the second obligation cannot be violated at
    // all, and `remove` restores from that same entry.
    unsafe { executor::set_stack_guard_hooks(install, remove) };
}

/// `(bands installed, install requests refused)` since boot.
pub fn stats() -> (usize, usize) {
    (
        INSTALLED.load(Ordering::Relaxed),
        REFUSED.load(Ordering::Relaxed),
    )
}

/// Claim a registry slot for `[base, base + size)`. `None` if the table is full.
fn claim_slot(base: usize, size: usize, flags: MMUFlags) -> Option<usize> {
    for i in 0..MAX_GUARDS {
        if GUARD_BASE[i].load(Ordering::Relaxed) != 0 {
            continue;
        }
        if GUARD_BASE[i]
            .compare_exchange(0, base, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            // Published last, and read only after `base` is seen: a concurrent
            // `is_guard_fault` that catches this slot mid-claim sees `end == 0`
            // and simply does not match, which is the safe direction.
            GUARD_FLAGS[i].store(flags.bits(), Ordering::Relaxed);
            GUARD_END[i].store(base + size, Ordering::Release);
            GUARD_HIGH.fetch_max(i + 1, Ordering::Release);
            return Some(i);
        }
    }
    None
}

/// Release a slot claimed by [`claim_slot`].
fn free_slot(i: usize) {
    GUARD_END[i].store(0, Ordering::Relaxed);
    GUARD_FLAGS[i].store(0, Ordering::Relaxed);
    GUARD_BASE[i].store(0, Ordering::Release);
}

/// Find the slot covering `vaddr`, if any.
///
/// On the kernel page-fault path, and that path is not rare — a fault taken in
/// kernel context on a *user* address (copy-to-user touching a page the VMAR
/// has not committed yet) comes through here too. Both early exits below exist
/// for that traffic: a guard band is always a kernel-half address, and only the
/// slots ever claimed are worth scanning.
fn slot_of(vaddr: usize) -> Option<usize> {
    if (vaddr as isize) >= 0 {
        return None;
    }
    let high = GUARD_HIGH.load(Ordering::Acquire).min(MAX_GUARDS);
    (0..high).find(|&i| {
        let base = GUARD_BASE[i].load(Ordering::Acquire);
        let end = GUARD_END[i].load(Ordering::Acquire);
        base != 0 && vaddr >= base && vaddr < end
    })
}

/// Write `flags` into every page of the band, reporting the first failure.
fn set_band_flags(pt: &mut PageTable, base: usize, size: usize, flags: MMUFlags) -> Result<(), ()> {
    for off in (0..size).step_by(PAGE_SIZE) {
        // `update_no_shootdown` because a synchronous shootdown per page would
        // be O(pages x ack-wait) on a path that runs on every executor
        // creation; the caller issues one remote flush for the whole band. The
        // *local* TLB is still invalidated per page by `update_no_shootdown`.
        if pt
            .update_no_shootdown(base + off, None, Some(flags))
            .is_err()
        {
            return Err(());
        }
    }
    Ok(())
}

/// Make `[guard_base, guard_base + guard_size)` fault on any access.
///
/// Returns `false` — having left the mapping exactly as it found it — if the
/// band is not a run of ordinary 4 KiB pages with uniform flags, if the
/// registry is full, or if the result does not verify. The scheduler then keeps
/// its soft canary.
fn install(guard_base: usize, guard_size: usize) -> bool {
    let refuse = |reason: &str| {
        let n = REFUSED.fetch_add(1, Ordering::Relaxed);
        // Logged only the first few times. This runs inside `Executor::new`,
        // which the scheduler calls with the runtime lock held and interrupts
        // off on every preemption-mid-poll — a per-executor log line from a
        // permanent condition (a full registry, say) would be a storm in
        // exactly the context that can least afford one. The scheduler prints
        // its own one-shot soft-canary fallback banner regardless, and
        // [`stats`] keeps the running count.
        if n < 4 {
            warn!(
                "stack_guard: refusing hard guard at {:#x}+{:#x} ({})",
                guard_base, guard_size, reason
            );
        }
        false
    };
    if guard_size == 0 || guard_base % PAGE_SIZE != 0 || guard_size % PAGE_SIZE != 0 {
        return refuse("band is not page aligned");
    }

    // The kernel half is shared by every address space (`pt_clone_kernel_space`
    // copies the top-level entries by value, so all of them walk into the same
    // sub-tables), which is why editing through whatever page table happens to
    // be loaded is enough — and necessary, since this can run under a user CR3
    // left behind by lazy TLB.
    let mut pt = PageTable::from_current();

    // Survey first, touch nothing: every page must be an ordinary 4 KiB
    // mapping, and all of them must agree on their flags so `remove` can
    // restore the band from a single recorded value.
    let expect = match pt.query(guard_base) {
        Ok((_, flags, size)) if size == crate::vm::PageSize::Size4K => flags,
        Ok((_, _, size)) => {
            return refuse(match size {
                crate::vm::PageSize::Size2M => "band is covered by a 2 MiB PTE",
                _ => "band is covered by a 1 GiB PTE",
            })
        }
        Err(_) => return refuse("band is not mapped"),
    };
    if expect.is_empty() {
        return refuse("band already has no permissions (double install?)");
    }
    for off in (0..guard_size).step_by(PAGE_SIZE) {
        match pt.query(guard_base + off) {
            // A page whose frame is physical 0 is refused, and not because it
            // could not be guarded: clearing the flags of such an entry leaves
            // it all-zero, which `is_unused()` reads as "no mapping here" — and
            // `update` refuses to touch an unused entry, so `remove` could
            // never put it back. No `.bss` page is ever backed by frame 0, so
            // this only ever fires on something already wrong.
            Ok((paddr, flags, crate::vm::PageSize::Size4K))
                if flags == expect && paddr & !(PAGE_SIZE - 1) != 0 => {}
            _ => return refuse("band is not a uniform run of mapped 4 KiB pages"),
        }
    }

    // Publish before editing: a fault inside the band from here on is a guard
    // hit and should be reported as one.
    let Some(slot) = claim_slot(guard_base, guard_size, expect) else {
        return refuse("guard registry is full");
    };

    let rollback = |pt: &mut PageTable| {
        // Best effort by construction: every entry still holds its own frame,
        // so restoring is one flag write per page and cannot fail for any
        // reason the survey above did not already rule out.
        let _ = set_band_flags(pt, guard_base, guard_size, expect);
        crate::vm::flush_tlb(None);
        crate::common::ipi::remote_flush_tlb_aspace(None, None);
    };

    if set_band_flags(&mut pt, guard_base, guard_size, MMUFlags::empty()).is_err() {
        rollback(&mut pt);
        free_slot(slot);
        return refuse("clearing the band's permissions failed");
    }

    // Verify rather than trust. The "empty flags means no present bit" property
    // is a per-architecture detail of `From<MMUFlags>`; reading it back is what
    // turns that into something this module knows rather than assumes. A
    // mismatch rolls back, so an architecture where it does not hold degrades
    // to the soft canary instead of shipping a guard band that does not guard.
    for off in (0..guard_size).step_by(PAGE_SIZE) {
        let gone = match pt.query(guard_base + off) {
            Ok((_, flags, _)) => flags.is_empty(),
            Err(_) => true,
        };
        if !gone {
            rollback(&mut pt);
            free_slot(slot);
            return refuse("band still readable after clearing its permissions");
        }
    }

    // One flush for the whole band. Other CPUs may hold TLB entries from the
    // heap's previous tenant at these addresses; without this the guard would
    // silently not exist on those cores. `aspace = None` deliberately targets
    // every CPU: a kernel mapping is reachable from every address space, so
    // filtering by page-table root would under-target.
    crate::common::ipi::remote_flush_tlb_aspace(None, None);
    INSTALLED.fetch_add(1, Ordering::Relaxed);
    true
}

/// Put a band installed by [`install`] back the way it was.
///
/// Called from `Executor::drop`, immediately before the allocation goes back to
/// the heap — so failing to restore would hand out memory with a hole in it,
/// and the next owner would fault on an address nothing explains. That is worth
/// a loud panic rather than a silent return.
fn remove(guard_base: usize, guard_size: usize) {
    let Some(slot) = slot_of(guard_base) else {
        // Never installed (the scheduler only calls this for bands it recorded
        // as hard, so this means the registry and the scheduler disagree).
        return;
    };
    let flags = MMUFlags::from_bits_truncate(GUARD_FLAGS[slot].load(Ordering::Relaxed));
    let mut pt = PageTable::from_current();
    if set_band_flags(&mut pt, guard_base, guard_size, flags).is_err() {
        panic!(
            "stack_guard: could not restore guard band {:#x}+{:#x} — refusing to \
             return unmapped memory to the heap",
            guard_base, guard_size
        );
    }
    // Other CPUs may have cached the not-present entry; make them re-walk.
    crate::vm::flush_tlb(None);
    crate::common::ipi::remote_flush_tlb_aspace(None, None);
    free_slot(slot);
}

/// Whether `fault_vaddr` fell inside an installed guard band.
///
/// The page-fault handler asks this before trying to resolve the fault against
/// the faulting thread's VMAR: a guard hit is a kernel stack overflow, and no
/// amount of VMAR work will explain it. Lock-free and allocation-free — it runs
/// on the fault path, where blocking would turn a reportable overflow into a
/// hang.
pub fn is_guard_fault(fault_vaddr: usize) -> bool {
    slot_of(fault_vaddr).is_some()
}
