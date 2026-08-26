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

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::vm::{GenericPageTable, PageTable};
use crate::MMUFlags;

const PAGE_SIZE: usize = 4096;

// ── Physical-frame registry: catch a physmap write that aliases a stack ───────
//
// The leading theory for the recurring null-range zeroing crash: a physical
// frame backing a VMO page physically aliases a live coroutine stack, and
// `VMObjectPaged::zero`'s `pmem_zero(paddr, ...)` (a physmap write) zeros the
// stack. The VA-based double-alloc tripwire cannot see this — the write lands
// through the physmap VA (`0xffff_8000 + paddr`), a *different* virtual address
// than the stack's kernel-image VA that maps the *same* physical frame.
//
// So track the PHYSICAL frames each live coroutine stack occupies, and let
// `pmem_zero`/`pmem_write` check their target against them. A hit is the
// smoking gun: a physmap write about to zero a live stack, caught with the
// writer's own call chain.
//
// A bitset over frame numbers, covering up to 16 GiB of RAM (4 Mi frames =
// 512 KiB static). Frames past that are not tracked (reported once): a machine
// with the kernel heap above 16 GiB is not a configuration this hunt targets.
const TRACKED_FRAMES: usize = 4 * 1024 * 1024; // 16 GiB / 4 KiB
const FRAME_WORDS: usize = TRACKED_FRAMES / 64;
static STACK_FRAME_BITS: [AtomicU64; FRAME_WORDS] = [const { AtomicU64::new(0) }; FRAME_WORDS];
static STACK_FRAMES_OVER_CAP: AtomicUsize = AtomicUsize::new(0);

#[inline]
fn frame_bit(frame: usize) -> (usize, u64) {
    (frame / 64, 1u64 << (frame % 64))
}

// ── Recently-freed DMA-block ring: catch the DEVICE/userspace-mapping UAF ─────
//
// The physmap guard above only catches a *CPU* write (`pmem_zero`/`pmem_write`)
// that aliases a live stack. It cannot see the two writers that reach a stack
// frame WITHOUT going through those primitives:
//
//   * a device DMA (a NIC RX ring, a GPU pushbuffer/GEM, an NVMe completion)
//     whose descriptor still points at a physical block after the driver freed
//     it — the device writes long after the CPU moved on;
//   * a userspace `VmObject::new_physical` mapping (the nouveau-uAPI GEM CPU
//     mmap: `gem_map_cpu` publishes `memdescGetPhysAddr`, Mesa mmaps it) that
//     outlives the GEM free.
//
// Both are the SAME shape: a physical block handed to a device / mapped into a
// process, then freed by `drivers_dma_dealloc` straight back to the general
// frame pool (no quarantine — see that function), then re-handed by
// `frame_alloc` to a fresh coroutine stack. `frame_alias_check` passes at that
// realloc (nothing lived there when the block was freed), and the stale
// mapping/descriptor then writes the recycled stack — the "all-zeros usable
// region, no guard hit" signature of the desktop-start crash on real NVIDIA
// hardware, where every zero-VRAM GEM travels this CPU-mmap path.
//
// This ring records the last N freed DMA blocks. The null-execute/soft-smash
// fault path asks `paddr_recently_freed_dma` whether the corrupted stack frame
// was one of them: a hit CONFIRMS the UAF and names it (a canary only ever said
// corruption *happened*, never that a freed DMA buffer was the writer). Pure
// diagnostic — recording is a couple of relaxed stores per DMA free, the lookup
// runs only on the already-fatal fault path.
const DMA_RING_SLOTS: usize = 512;
static DMA_FREED_BASE: [AtomicUsize; DMA_RING_SLOTS] =
    [const { AtomicUsize::new(0) }; DMA_RING_SLOTS];
static DMA_FREED_PAGES: [AtomicUsize; DMA_RING_SLOTS] =
    [const { AtomicUsize::new(0) }; DMA_RING_SLOTS];
/// Monotonic count of DMA-block frees since boot; also the ring write cursor.
static DMA_FREED_SEQ: AtomicU64 = AtomicU64::new(0);

/// Record that `[paddr, paddr + pages*PAGE)` was just freed by a DMA path.
/// Called from `drivers_dma_dealloc` for every block returned to the pool.
pub fn dma_free_note(paddr: usize, pages: usize) {
    if pages == 0 {
        return;
    }
    let seq = DMA_FREED_SEQ.fetch_add(1, Ordering::AcqRel);
    let slot = (seq as usize) % DMA_RING_SLOTS;
    // Base published last: a concurrent reader that catches this slot mid-write
    // sees the OLD (base, pages) or a zero pages and simply does not match — the
    // safe direction for a best-effort diagnostic.
    DMA_FREED_PAGES[slot].store(pages, Ordering::Relaxed);
    DMA_FREED_BASE[slot].store(paddr, Ordering::Release);
}

/// If `paddr` falls inside a recently-freed DMA block, return how many DMA
/// frees have happened since (0 = the most recent). `None` if not found —
/// either it was never a DMA buffer, or it aged out of the ring.
pub fn paddr_recently_freed_dma(paddr: usize) -> Option<u64> {
    let now = DMA_FREED_SEQ.load(Ordering::Acquire);
    for back in 0..(DMA_RING_SLOTS as u64) {
        if back >= now {
            break;
        }
        let seq = now - 1 - back;
        let slot = (seq as usize) % DMA_RING_SLOTS;
        let base = DMA_FREED_BASE[slot].load(Ordering::Acquire);
        let pages = DMA_FREED_PAGES[slot].load(Ordering::Relaxed);
        if pages == 0 {
            continue;
        }
        if paddr >= base && paddr < base + pages * PAGE_SIZE {
            return Some(back);
        }
    }
    None
}

/// Mark or clear every physical frame backing the usable stack `[usable_base,
/// usable_base + STACK_SIZE)` in the stack-frame bitset, by querying the live
/// page table for each page's physical address.
fn mark_stack_frames(usable_base: usize, set: bool) {
    let pt = PageTable::from_current();
    let stack_size = executor::STACK_SIZE;
    for off in (0..stack_size).step_by(PAGE_SIZE) {
        let Ok((paddr, _, _)) = pt.query(usable_base + off) else {
            continue;
        };
        let frame = paddr / PAGE_SIZE;
        if frame >= TRACKED_FRAMES {
            STACK_FRAMES_OVER_CAP.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        let (w, b) = frame_bit(frame);
        if set {
            STACK_FRAME_BITS[w].fetch_or(b, Ordering::Relaxed);
        } else {
            STACK_FRAME_BITS[w].fetch_and(!b, Ordering::Relaxed);
        }
    }
}

/// Physmap-write guard: report (once) if `who` is about to write over a live
/// coroutine stack through the physmap, and name the writing call chain.
///
/// This is THE check for the leading root-cause theory. A hit means a physical
/// frame backing a VMO page (or a DMA buffer, or a `pmem_copy` destination)
/// physically aliases a live executor stack — the wild zero-writer, caught at
/// the instant of the write with its own backtrace. Latches the smash flag so
/// the timer path stops running on the (about-to-be) corrupted stack.
pub fn check_physmap_write(who: &str, paddr: usize, len: usize) {
    if !paddr_aliases_stack(paddr, len) {
        return;
    }
    ::executor::note_heap_smash_suspected();
    use core::sync::atomic::AtomicBool;
    static REPORTED: AtomicBool = AtomicBool::new(false);
    if REPORTED.swap(true, Ordering::SeqCst) {
        return;
    }
    crate::console::serial_write_fmt_spin(format_args!(
        "\n[physmap-smash] {} paddr={:#x} len={:#x} ALIASES A LIVE COROUTINE STACK — \
         this physmap write is the wild zero-writer (diag rev 5). Call chain:\n",
        who, paddr, len,
    ));
    // Frame-pointer backtrace of the writing path, bounded and guarded.
    #[cfg(target_arch = "x86_64")]
    {
        let mut rbp: usize;
        unsafe { core::arch::asm!("mov {}, rbp", out(reg) rbp) };
        for _ in 0..24 {
            if rbp == 0 || rbp & 0x7 != 0 || rbp < 0xffff_ff00_0000_0000 {
                break;
            }
            let ret = unsafe { core::ptr::read_volatile((rbp + 8) as *const usize) };
            let next = unsafe { core::ptr::read_volatile(rbp as *const usize) };
            if ret == 0 {
                break;
            }
            crate::console::serial_write_fmt_spin(format_args!(
                "[physmap-smash]   ret={:#x}\n",
                ret
            ));
            if next <= rbp {
                break;
            }
            rbp = next;
        }
    }
    crate::console::serial_write_str(
        "[physmap-smash] symbolize: llvm-addr2line -e <zcore.elf> -fCi <ret ...>\n",
    );
}

/// Whether any physical frame in `[paddr, paddr + len)` currently backs a live
/// coroutine stack. Called by the physmap write primitives (`pmem_zero`,
/// `pmem_write`) before they scribble — a `true` means the write would corrupt
/// a live executor stack through the physmap alias.
pub fn paddr_aliases_stack(paddr: usize, len: usize) -> bool {
    if len == 0 {
        return false;
    }
    let first = paddr / PAGE_SIZE;
    let last = (paddr + len - 1) / PAGE_SIZE;
    for frame in first..=last {
        if frame >= TRACKED_FRAMES {
            continue;
        }
        let (w, b) = frame_bit(frame);
        if STACK_FRAME_BITS[w].load(Ordering::Relaxed) & b != 0 {
            return true;
        }
    }
    false
}

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
    // SAFETY: same contract — `quarantine_unprotect` restores the original
    // flags (kept in the registry) before the scheduler frees the VA, and no
    // frame ever leaves its page-table entry. Only used when STACKQUARANTINE=1
    // arms the scheduler side.
    unsafe { executor::set_stack_quarantine_hooks(quarantine_protect, quarantine_unprotect) };
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
    // The bottom guard install is the one point that knows this executor's
    // full layout: record the usable stack's physical frames so a physmap
    // write that aliases them is caught. `guard_base` is `alloc_base`, so the
    // usable region starts one bottom-guard above it.
    if guard_size == executor::GUARD_SIZE {
        mark_stack_frames(guard_base + executor::GUARD_SIZE, true);
    }
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
    // Stop tracking this stack's physical frames before the memory is freed:
    // the frames are about to be legitimately reused, and a stale bit would
    // make the next VMO zeroing of them a false positive.
    if guard_size == executor::GUARD_SIZE {
        mark_stack_frames(guard_base + executor::GUARD_SIZE, false);
    }
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

// ── Freed-stack quarantine: catch the use-after-free WRITER red-handed ────────
//
// The recurring coroutine-stack smash (rev 7 run: a transient executor's return
// slot zeroed, then a `ret` into `rip=0x0`) is a heap use-after-free that no
// dispatch gate can catch — it is a raw write, not a `dyn` call. The only way to
// name the culprit is to catch the write itself.
//
// So when a transient executor's stack is freed, the scheduler does not return
// it to the heap immediately: it hands the usable region here to be
// **write-protected** (present + readable, but not writable) and held in a
// bounded ring. A dangling pointer that still writes into the freed stack then
// takes a clean WRITE #PF *at the writer's own rip*, which `zcore::handler`
// reports as `[stack-uaf]` with the writing call chain — the exact instruction
// doing the UAF, instead of the damage discovered later on the victim's stack.
//
// Write-protect (not unmap) on purpose: a benign stale *read* of freed memory
// is not the corruptor and should not fault, but every stale *write* — the zero
// writer — does. Same PTE machinery, TLB flush and lock-free registry as the
// guards; a separate table so a hit is reported as a UAF, not an overflow.

/// Same real ceiling as the guard registry — a handful of executors churn at
/// once, and the scheduler's ring holds only the most-recently-freed stacks.
const MAX_QUAR: usize = 128;
static QUAR_BASE: [AtomicUsize; MAX_QUAR] = [const { AtomicUsize::new(0) }; MAX_QUAR];
static QUAR_END: [AtomicUsize; MAX_QUAR] = [const { AtomicUsize::new(0) }; MAX_QUAR];
/// Original flags to write back when the stack is finally released.
static QUAR_FLAGS: [AtomicUsize; MAX_QUAR] = [const { AtomicUsize::new(0) }; MAX_QUAR];
static QUAR_HIGH: AtomicUsize = AtomicUsize::new(0);
static QUAR_PROTECTED: AtomicUsize = AtomicUsize::new(0);

fn quar_claim_slot(base: usize, size: usize, flags: MMUFlags) -> Option<usize> {
    for i in 0..MAX_QUAR {
        if QUAR_BASE[i].load(Ordering::Relaxed) != 0 {
            continue;
        }
        if QUAR_BASE[i]
            .compare_exchange(0, base, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            QUAR_FLAGS[i].store(flags.bits(), Ordering::Relaxed);
            QUAR_END[i].store(base + size, Ordering::Release);
            QUAR_HIGH.fetch_max(i + 1, Ordering::Release);
            return Some(i);
        }
    }
    None
}

fn free_quar_slot(i: usize) {
    QUAR_END[i].store(0, Ordering::Relaxed);
    QUAR_FLAGS[i].store(0, Ordering::Relaxed);
    QUAR_BASE[i].store(0, Ordering::Release);
}

fn quar_slot_of(vaddr: usize) -> Option<usize> {
    if (vaddr as isize) >= 0 {
        return None;
    }
    let high = QUAR_HIGH.load(Ordering::Acquire).min(MAX_QUAR);
    (0..high).find(|&i| {
        let base = QUAR_BASE[i].load(Ordering::Acquire);
        let end = QUAR_END[i].load(Ordering::Acquire);
        base != 0 && vaddr >= base && vaddr < end
    })
}

/// Write-protect the freed usable stack `[usable_base, usable_base + size)` and
/// register it, so a stale write into it faults at the writer.
///
/// Returns `false` — having touched nothing — if the region is not a uniform run
/// of writable 4 KiB pages or the registry is full; the scheduler then frees the
/// stack the ordinary way. Same all-or-nothing contract as [`install`].
pub fn quarantine_protect(usable_base: usize, size: usize) -> bool {
    if size == 0 || usable_base % PAGE_SIZE != 0 || size % PAGE_SIZE != 0 {
        return false;
    }
    let mut pt = PageTable::from_current();
    let expect = match pt.query(usable_base) {
        Ok((_, flags, crate::vm::PageSize::Size4K)) => flags,
        // Huge PTE or unmapped: cannot protect at page granularity.
        _ => return false,
    };
    if !expect.contains(MMUFlags::WRITE) || expect.is_empty() {
        return false;
    }
    for off in (0..size).step_by(PAGE_SIZE) {
        match pt.query(usable_base + off) {
            Ok((paddr, flags, crate::vm::PageSize::Size4K))
                if flags == expect && paddr & !(PAGE_SIZE - 1) != 0 => {}
            _ => return false,
        }
    }
    let Some(slot) = quar_claim_slot(usable_base, size, expect) else {
        return false;
    };
    // Present + readable, minus WRITE: a stale read stays silent, a stale write
    // faults with the WRITE error bit set.
    let readonly = MMUFlags::from_bits_truncate(expect.bits() & !MMUFlags::WRITE.bits());
    if set_band_flags(&mut pt, usable_base, size, readonly).is_err() {
        let _ = set_band_flags(&mut pt, usable_base, size, expect);
        crate::vm::flush_tlb(None);
        crate::common::ipi::remote_flush_tlb_aspace(None, None);
        free_quar_slot(slot);
        return false;
    }
    crate::vm::flush_tlb(None);
    crate::common::ipi::remote_flush_tlb_aspace(None, None);
    QUAR_PROTECTED.fetch_add(1, Ordering::Relaxed);
    true
}

/// Undo [`quarantine_protect`] on `[usable_base, usable_base + size)`, restoring
/// the original flags just before the scheduler finally frees the stack. Loud
/// panic on failure: returning write-protected memory to the heap would fault
/// the next owner on an address nothing explains.
pub fn quarantine_unprotect(usable_base: usize, size: usize) {
    let Some(slot) = quar_slot_of(usable_base) else {
        return;
    };
    let flags = MMUFlags::from_bits_truncate(QUAR_FLAGS[slot].load(Ordering::Relaxed));
    let mut pt = PageTable::from_current();
    if set_band_flags(&mut pt, usable_base, size, flags).is_err() {
        panic!(
            "stack_guard: could not un-protect quarantined stack {:#x}+{:#x} — \
             refusing to return write-protected memory to the heap",
            usable_base, size
        );
    }
    crate::vm::flush_tlb(None);
    crate::common::ipi::remote_flush_tlb_aspace(None, None);
    free_quar_slot(slot);
}

/// Whether `fault_vaddr` fell inside a write-protected quarantined stack — i.e.
/// a use-after-free write into freed coroutine-stack memory. Lock-free and
/// allocation-free; safe on the page-fault path.
pub fn is_quarantine_fault(fault_vaddr: usize) -> bool {
    quar_slot_of(fault_vaddr).is_some()
}

/// `(stacks currently/ever quarantined-protected)` for the boot/diagnostic log.
pub fn quarantine_stats() -> usize {
    QUAR_PROTECTED.load(Ordering::Relaxed)
}
