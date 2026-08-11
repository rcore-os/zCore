use crate::context::{Context as ExecuterContext, ContextData};
use alloc::alloc::{Allocator, Global, Layout};
use core::pin::Pin;
use {
    alloc::boxed::Box,
    alloc::sync::Arc,
    core::ptr::NonNull,
    core::task::{Context, Poll},
};

use crate::arch::executor_entry;
use crate::task_collection::{Task, TaskCollection};
use crate::waker_page::WakerRef;
use core::sync::atomic::AtomicBool;

#[derive(Debug, PartialEq, Eq)]
enum ExecutorState {
    STRONG,
    WEAK, // 执行完一次future后就需要被drop
    KILLED,
    UNUSED,
}

pub struct Executor {
    id: usize,
    task_collection: Arc<TaskCollection>,
    stack_base: usize,
    /// Bottom soft-guard region unmapped via [`set_stack_guard_hooks`].
    hard_guard_bottom: bool,
    /// Top guard (above usable stack) unmapped — catches a neighbour growing down.
    hard_guard_top: bool,
    pub context: ExecuterContext,
    #[cfg(any(target_arch = "riscv64", target_arch = "aarch64"))]
    context_data: ContextData,
    task_id: usize,
    state: ExecutorState,
    /// The task checked out for the poll currently in flight, with its waker.
    /// The panic-containment path needs to name — and retire — the future that
    /// was running when a fault hit this executor's stack, and `task_id` alone
    /// cannot do that.
    ///
    /// Raw pointers rather than `Arc` clones on purpose: this is the hot poll
    /// path, where the surrounding code goes out of its way to avoid refcount
    /// round-trips (see `Task::waker`'s doc). Both are set immediately before
    /// `Task::poll` and cleared immediately after, and the `Arc`s they borrow
    /// from are live locals of `run` across that whole window — including while
    /// the poll is parked by preemption. The only reader is
    /// [`Executor::abandon_current_task`], reached from a fault taken *inside*
    /// that poll on this same CPU, which is exactly when they are valid.
    current_task: *const Task,
    current_waker: *const WakerRef,
    /// Set when this executor's coroutine stack was abandoned mid-poll (see
    /// [`Executor::abandon_current_task`]). Separate from `state` because it is
    /// written through a shared `&Executor` while the runtime still holds its
    /// `Arc`, and because it must survive the `WEAK` transition that
    /// `downgrade_strong_executor` performs afterwards.
    abandoned: AtomicBool,
    /// Forces the runtime to REPLACE this executor instead of resuming it.
    ///
    /// Set by [`abandon_idle_executor`](Self::abandon_idle_executor): a fault
    /// taken between polls has no task to blame, so `task_id` is already 0 and
    /// `is_running_future()` would say "nothing in flight, just resume it" —
    /// straight back onto the faulting instruction on a corrupt stack. This
    /// flag makes the runtime take the replace path instead.
    force_replace: AtomicBool,
}

/// Idle-loop iterations since any task was last polled (hang detector; see the
/// idle branch in `run`). Global because all executors on a CPU share progress.
static IDLE_STREAK: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

// Scheduler-loop branch counters, surfaced via [`sched_stats`] for
// `/proc/perf/kernel` so a busy-spin can be attributed: `polled` = a task was
// available to run, `weak_yield` = no task but a weak executor outstanding so we
// spun via `sched_yield` instead of halting.
static SCHED_POLLED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static SCHED_WEAK_YIELD: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// `(tasks polled, weak-executor yields)` since boot.
pub fn sched_stats() -> (u64, u64) {
    use core::sync::atomic::Ordering::Relaxed;
    (SCHED_POLLED.load(Relaxed), SCHED_WEAK_YIELD.load(Relaxed))
}

const PAGE_SIZE: usize = 4096;
/// Soft / hard guard **below** the usable stack. Soft path fills canary words;
/// hard path (when hooks are registered) unmaps these pages so overflow #PFs
/// instead of smashing neighbouring heap (`rip=0x3` / `[rsp0]=0x13486`).
///
/// 512 KiB (was 128 KiB / 16 KiB). Even with `"stack-probes": {"kind": "inline"}`
/// in `zCore/x86_64.json`, keep a wide bottom hole so a missed probe / IRQ nest
/// still #PFs before neighbour heap smash. Proven: hard_guard_executors=64 still
/// soft-smashed with 128 KiB (hole jumped or neighbour smash) before probes
/// were enabled on the custom target.
pub const GUARD_SIZE: usize = PAGE_SIZE * 128; // 512 KiB
/// Unmapped region **above** the usable stack. A neighbour stack growing down
/// hits this before smashing this stack's high return-address slots.
pub const TOP_GUARD_SIZE: usize = PAGE_SIZE * 16; // 64 KiB
/// 2 MiB usable coroutine stack. History: 256 KiB and 512 KiB overflowed into
/// neighbouring heap (`rip=0x0` / `[rsp0]=0x13446`); 1 MiB still saw near-
/// overflow at labwc/lunarbar desktop start (timer smash → `rip=0x3`). Extra
/// headroom is cheap relative to a guard-page-less smash.
///
/// Per-executor footprint: `STACK_SIZE + GUARD_SIZE + TOP_GUARD_SIZE`
/// (= 2 MiB + 512 KiB + 64 KiB). 64 executors ≈ 160 MiB — fine in a 512 MiB heap.
pub const STACK_SIZE: usize = 4096 * 512;
const ALLOC_SIZE: usize = STACK_SIZE + GUARD_SIZE + TOP_GUARD_SIZE;
// Page-aligned so the hard-guard path can `unmap` 4K PTEs in the BSS heap.
const ALLOC_LAYOUT: Layout = match Layout::from_size_align(ALLOC_SIZE, PAGE_SIZE) {
    Ok(l) => l,
    Err(_) => unreachable!(),
};

/// Magic written across the soft-guard region below every coroutine stack.
/// Layout: `[BOTTOM_GUARD GUARD_SIZE][usable STACK_SIZE][TOP_GUARD TOP_GUARD_SIZE]`.
/// Usable grows down from `stack_base + STACK_SIZE` toward `stack_base`; bottom
/// guard is `[stack_base - GUARD_SIZE, stack_base)`; top guard is
/// `[stack_base + STACK_SIZE, stack_base + STACK_SIZE + TOP_GUARD_SIZE)`.
const STACK_CANARY: u64 = 0x5354_4143_4b5f_4f56; // "STACK_OV"
const GUARD_WORDS: usize = GUARD_SIZE / core::mem::size_of::<u64>();

// ── Live coroutine-stack registry (double-alloc tripwire) ────────────────────
//
// Every crash capture zeroes a run of a *transient* executor's stack (exec
// ids 2,3,4 — the downgraded ones whose stacks are freed and reused — never
// the immortal 0/1), and the fill is zeros, not the `0xDEADBEEF` poison
// `Executor::new` writes. That is the signature of a plain zero-initialised
// heap buffer (`vec![0; n]`, a zeroed Box) handed out by the buddy allocator
// over memory that is STILL a live coroutine stack — a heap double-allocation
// / use-after-free of a stack.
//
// This registry records every live stack allocation; the kernel's global
// allocator calls [`alloc_overlaps_live_stack`] on each block it hands out, so
// the double-alloc is caught at the moment it is dispensed, with the
// allocating call chain (the writer's own path) on the stack. Lock-free by
// construction (atomics only): it is consulted from inside the allocator lock.
const STACK_REG_SLOTS: usize = 256;
static STACK_REG_BASE: [core::sync::atomic::AtomicUsize; STACK_REG_SLOTS] =
    [const { core::sync::atomic::AtomicUsize::new(0) }; STACK_REG_SLOTS];

/// Record `[alloc_base, alloc_base + ALLOC_SIZE)` as a live stack. `base == 0`
/// slots are free. Silently no-ops if the table is full (only lowers coverage).
fn stack_reg_insert(alloc_base: usize) {
    use core::sync::atomic::Ordering::AcqRel;
    for slot in STACK_REG_BASE.iter() {
        if slot
            .compare_exchange(0, alloc_base, AcqRel, core::sync::atomic::Ordering::Relaxed)
            .is_ok()
        {
            return;
        }
    }
}

/// Remove a stack recorded by [`stack_reg_insert`].
fn stack_reg_remove(alloc_base: usize) {
    use core::sync::atomic::Ordering::AcqRel;
    for slot in STACK_REG_BASE.iter() {
        if slot
            .compare_exchange(alloc_base, 0, AcqRel, core::sync::atomic::Ordering::Relaxed)
            .is_ok()
        {
            return;
        }
    }
}

/// Whether `[ptr, ptr + len)` overlaps any live coroutine-stack allocation.
///
/// Returns the overlapping `alloc_base` if so. The global allocator calls this
/// on every block it dispenses: a hit means the buddy handed out memory that is
/// still a live executor stack — the double-alloc these crashes are chasing.
///
/// Excludes an exact `alloc_base` match with `len >= ALLOC_SIZE`, which is the
/// legitimate case of `Executor::new` itself re-allocating a slot the registry
/// has not recorded yet (insert happens after the allocation returns).
pub fn alloc_overlaps_live_stack(ptr: usize, len: usize) -> Option<usize> {
    use core::sync::atomic::Ordering::Acquire;
    let a_end = ptr.wrapping_add(len);
    for slot in STACK_REG_BASE.iter() {
        let base = slot.load(Acquire);
        if base == 0 {
            continue;
        }
        let b_end = base + ALLOC_SIZE;
        if ptr < b_end && base < a_end {
            return Some(base);
        }
    }
    None
}

/// Optional hard-guard install/remove. Registered by kernel-hal once page
/// tables are ready. `install` returns false to keep the soft canary.
type StackGuardHook = fn(guard_base: usize, guard_size: usize) -> bool;
type StackGuardRemove = fn(guard_base: usize, guard_size: usize);

static STACK_GUARD_INSTALL: spin::Mutex<Option<StackGuardHook>> = spin::Mutex::new(None);
static STACK_GUARD_REMOVE: spin::Mutex<Option<StackGuardRemove>> = spin::Mutex::new(None);

/// Executors created with an unmapped (hard) guard vs soft canary.
static HARD_GUARD_EXECUTORS: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);
static SOFT_GUARD_EXECUTORS: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);

/// `(hard_guard count, soft_guard count)` of executors created this boot.
pub fn hard_guard_executor_counts() -> (usize, usize) {
    use core::sync::atomic::Ordering::Relaxed;
    (
        HARD_GUARD_EXECUTORS.load(Relaxed),
        SOFT_GUARD_EXECUTORS.load(Relaxed),
    )
}

/// Register unmapped-guard hooks. Safe to call once at boot; later calls replace.
///
/// # Safety
/// Hooks must remap frames on remove before the VA is returned to the heap,
/// and must not free BSS-owned physical frames to the frame allocator.
pub unsafe fn set_stack_guard_hooks(install: StackGuardHook, remove: StackGuardRemove) {
    *STACK_GUARD_INSTALL.lock() = Some(install);
    *STACK_GUARD_REMOVE.lock() = Some(remove);
}

/// True once [`set_stack_guard_hooks`] has registered install/remove.
pub fn stack_guard_hooks_registered() -> bool {
    STACK_GUARD_INSTALL.lock().is_some()
}

// ── Freed-stack quarantine (diagnostic; armed by STACKQUARANTINE=1) ───────────
//
// Instead of returning a freed coroutine stack straight to the heap, hold it
// write-protected in a bounded ring so a dangling pointer that writes into it
// faults at the writer's rip (see `kernel_hal::stack_guard`'s quarantine). This
// pins the use-after-free that smashes transient-executor stacks. Off by default
// because it holds `QUAR_RING` allocations back and adds a TLB shootdown per
// free — both fine for a diagnostic run, not for normal operation.

/// Protect a freed usable stack; returns false if it could not (caller frees
/// normally). Unprotect restores it just before the real free.
type StackQuarProtect = fn(usable_base: usize, size: usize) -> bool;
type StackQuarUnprotect = fn(usable_base: usize, size: usize);

static STACK_QUAR_PROTECT: spin::Mutex<Option<StackQuarProtect>> = spin::Mutex::new(None);
static STACK_QUAR_UNPROTECT: spin::Mutex<Option<StackQuarUnprotect>> = spin::Mutex::new(None);

static QUARANTINE_ENABLED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// How many freed stacks to hold write-protected at once. Each is
/// `ALLOC_SIZE` (~2.6 MiB); the ring bounds the memory held out of the heap.
const QUAR_RING: usize = 24;
static QUAR_RING_SLOTS: [core::sync::atomic::AtomicUsize; QUAR_RING] =
    [const { core::sync::atomic::AtomicUsize::new(0) }; QUAR_RING];
static QUAR_RING_IDX: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// Register the quarantine protect/unprotect hooks (kernel-hal, bare-metal).
///
/// # Safety
/// `unprotect` must restore the exact flags `protect` cleared before the VA is
/// freed, and neither may hand a `.bss` frame to the frame allocator.
pub unsafe fn set_stack_quarantine_hooks(protect: StackQuarProtect, unprotect: StackQuarUnprotect) {
    *STACK_QUAR_PROTECT.lock() = Some(protect);
    *STACK_QUAR_UNPROTECT.lock() = Some(unprotect);
}

/// Arm/disarm the freed-stack quarantine (STACKQUARANTINE=1 at boot).
pub fn set_stack_quarantine_enabled(on: bool) {
    QUARANTINE_ENABLED.store(on, core::sync::atomic::Ordering::SeqCst);
}

/// Push a just-freed `alloc_base` into the ring; returns the `alloc_base` it
/// evicted (`0` if the slot was empty), which the caller must unprotect + free.
fn quar_ring_push(alloc_base: usize) -> usize {
    use core::sync::atomic::Ordering;
    let idx = QUAR_RING_IDX.fetch_add(1, Ordering::Relaxed) % QUAR_RING;
    QUAR_RING_SLOTS[idx].swap(alloc_base, Ordering::AcqRel)
}

fn executor_alloc_id() -> usize {
    use core::sync::atomic::{AtomicUsize, Ordering};
    static EXECUTOR_ID: AtomicUsize = AtomicUsize::new(1);
    EXECUTOR_ID.fetch_add(1, Ordering::SeqCst)
}

fn note_first_executor_created(hard_guard_bottom: bool, hard_guard_top: bool) {
    use core::sync::atomic::{AtomicBool, Ordering};
    static LOGGED: AtomicBool = AtomicBool::new(false);
    if LOGGED.swap(true, Ordering::SeqCst) {
        return;
    }
}

fn note_soft_guard_fallback(reason: &'static str) {
    use core::sync::atomic::{AtomicBool, Ordering};
    static LOGGED: AtomicBool = AtomicBool::new(false);
    if LOGGED.swap(true, Ordering::SeqCst) {
        return;
    }
    // Soft canary still detects overflow, but later than an unmapped guard —
    // desktop bring-up previously reached rip=0 / [rsp0]=0x13446 first.
    error!(
        "stack_guard: HARD GUARD UNAVAILABLE ({}) — falling back to soft canary; \
         overflow may smash neighbouring heap before the next IRQ tripwire \
         (rip=0 / [rsp0]=0x13446 class)",
        reason
    );
}

/// [diag] Lock-free registry of live coroutine-stack allocations.
///
/// Kernel coroutine stacks and userspace VMO frames come out of the SAME
/// buddy arena (`zCore::memory`: `frame_alloc` and the `GlobalAlloc` impl both
/// call `HEAP.0.lock().allocate`). If that allocator ever hands the same block
/// to both, the consequences match the crashes in
/// `docs/README-crash-repro.md` exactly: a freshly created VMO is zero-filled,
/// which would blank a live kernel stack (`rip=0x0`, `[rsp0..3]=0`,
/// `region=usable` — an overflow would have faulted in the unmapped guard
/// instead), and userspace then writing its buffer sprays arbitrary bytes over
/// kernel stacks and heap objects (the mangled return addresses and the
/// clobbered `KObjectBase` name `String`).
///
/// Walking `GLOBAL_RUNTIME` to test that costs a `try_lock` per CPU, far too
/// much for an allocator hot path — and heavy enough to shift the timing that
/// reproduces the bug. This registry is plain atomics: registering is one
/// store, and a check is a handful of relaxed loads.
const MAX_TRACKED_STACKS: usize = 128;
static STACK_REG: [core::sync::atomic::AtomicUsize; MAX_TRACKED_STACKS] =
    [const { core::sync::atomic::AtomicUsize::new(0) }; MAX_TRACKED_STACKS];
/// Live stacks that did not fit `STACK_REG` (the check is then incomplete;
/// reported so a silent gap is never mistaken for a clean result).
static STACK_REG_OVERFLOW: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);

/// Publish `[alloc_base, alloc_base + ALLOC_SIZE)` as a live stack.
fn register_stack(alloc_base: usize) {
    for slot in STACK_REG.iter() {
        if slot
            .compare_exchange(0, alloc_base, core::sync::atomic::Ordering::AcqRel, core::sync::atomic::Ordering::Relaxed)
            .is_ok()
        {
            return;
        }
    }
    STACK_REG_OVERFLOW.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
}

/// Retract a stack registered by [`register_stack`].
fn unregister_stack(alloc_base: usize) {
    for slot in STACK_REG.iter() {
        if slot
            .compare_exchange(alloc_base, 0, core::sync::atomic::Ordering::AcqRel, core::sync::atomic::Ordering::Relaxed)
            .is_ok()
        {
            return;
        }
    }
}

/// [diag] Does `[start, start + len)` overlap a live coroutine-stack
/// allocation (guards included)? Returns the offending stack's alloc base.
///
/// Intended for the allocator to call on every block it is about to hand out:
/// an overlap means the block is already spoken for, and reporting it *at
/// hand-out* names the aliasing before any corruption happens — unlike a
/// canary or a watchpoint, which can only report damage after the fact.
pub fn overlapping_live_stack(start: usize, len: usize) -> Option<usize> {
    let end = start.checked_add(len)?;
    for slot in STACK_REG.iter() {
        let base = slot.load(core::sync::atomic::Ordering::Acquire);
        if base != 0 && start < base + ALLOC_SIZE && base < end {
            return Some(base);
        }
    }
    None
}

/// [diag] Live stacks that could not be tracked; non-zero means
/// [`overlapping_live_stack`] can return false negatives.
pub fn untracked_live_stacks() -> usize {
    STACK_REG_OVERFLOW.load(core::sync::atomic::Ordering::Relaxed)
}

impl Executor {
    pub fn new(task_collection: Arc<TaskCollection>) -> Pin<Box<Self>> {
        let raw: NonNull<u8> = Global
            .allocate(ALLOC_LAYOUT)
            .expect("Alloction Stack Failed.")
            .cast();
        let alloc_base = raw.as_ptr() as usize;
        debug_assert_eq!(alloc_base % PAGE_SIZE, 0);
        register_stack(alloc_base);
        let stack_base = alloc_base + GUARD_SIZE;
        let top_guard_base = stack_base + STACK_SIZE;
        // Prefer unmapped guards (hard). Soft canary only if hooks are missing
        // or install refuses (e.g. huge-page leaf). Bare-metal boot must call
        // `stack_guard::init` before the first `Executor::new`
        // (`warm_runtimes` right after hooks).
        //
        // Install bottom and top separately (hook API is one range per call;
        // stack_guard stores each base independently).
        let (hard_guard_bottom, hard_guard_top) = match *STACK_GUARD_INSTALL.lock() {
            Some(f) => {
                let ok_bottom = f(alloc_base, GUARD_SIZE);
                if !ok_bottom {
                    note_soft_guard_fallback("bottom install refused (huge PTE / unmap failed)");
                }
                let ok_top = f(top_guard_base, TOP_GUARD_SIZE);
                if !ok_top {
                    note_soft_guard_fallback("top install refused (huge PTE / unmap failed)");
                }
                (ok_bottom, ok_top)
            }
            None => {
                note_soft_guard_fallback("hooks not registered yet");
                (false, false)
            }
        };
        if !hard_guard_bottom {
            unsafe {
                let p = alloc_base as *mut u64;
                for i in 0..GUARD_WORDS {
                    core::ptr::write_volatile(p.add(i), STACK_CANARY ^ i as u64);
                }
            }
            SOFT_GUARD_EXECUTORS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        } else {
            HARD_GUARD_EXECUTORS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        }
        // Poison usable stack before context is written on top. Fresh buddy
        // pages are BSS-zero; a RET into never-used slots was rip=0 / [rsp]=0.
        // Non-zero poison makes that a distinctive bad-RIP #PF instead.
        unsafe {
            let p = stack_base as *mut u64;
            let words = STACK_SIZE / core::mem::size_of::<u64>();
            const STACK_POISON: u64 = 0xDEAD_BEEF_DEAD_BEEF;
            for i in 0..words {
                core::ptr::write_volatile(p.add(i), STACK_POISON);
            }
        }
        note_first_executor_created(hard_guard_bottom, hard_guard_top);
        // Record this stack as live BEFORE returning it, so the allocator's
        // double-alloc check sees it from the first instant it can be aliased.
        stack_reg_insert(alloc_base);
        let mut pin_executor = Pin::new(Box::new(Executor {
            id: executor_alloc_id(),
            task_collection,
            stack_base,
            hard_guard_bottom,
            hard_guard_top,
            context: ExecuterContext::default(),
            #[cfg(any(target_arch = "riscv64", target_arch = "aarch64"))]
            context_data: ContextData::default(),
            task_id: 0,
            state: ExecutorState::UNUSED,
            current_task: core::ptr::null(),
            current_waker: core::ptr::null(),
            abandoned: AtomicBool::new(false),
            force_replace: AtomicBool::new(false),
        }));

        pin_executor.init_stack_and_context();

        trace!(
            "stack top 0x{:x} executor addr 0x{:x}, pgbr = 0x{:x}",
            pin_executor.context.get_sp(),
            pin_executor.context.get_pc(),
            pin_executor.context.get_pgbr(),
        );
        pin_executor
    }

    // stack layout: [executor_addr | context ]
    fn init_stack_and_context(&mut self) {
        let mut stack_top = self.stack_base + STACK_SIZE;
        let self_addr = self as *const Self as usize;
        stack_top = unsafe { push_stack(stack_top, self_addr) };
        #[cfg(any(target_arch = "riscv64", target_arch = "aarch64"))]
        {
            self.context_data = ContextData::new(
                executor_entry as *const () as usize,
                stack_top,
                crate::arch::pg_base_register(),
            );
            self.context
                .set_context(&self.context_data as *const _ as usize);
        }
        #[cfg(target_arch = "x86_64")]
        {
            let context_data = ContextData::new(
                executor_entry as *const () as usize,
                stack_top,
                crate::arch::pg_base_register(),
            );
            stack_top = unsafe { push_stack(stack_top, context_data) };
            self.context.set_context(stack_top);
        }
    }

    pub fn run(&mut self) {
        // Lazy-TLB safety pin.
        //
        // `ThreadSwitchFuture::poll` leaves this CPU on the polled thread's
        // *process* page table after the poll (lazy-TLB: it skips reloading the
        // kernel CR3 to avoid a TLB flush per poll). The scheduler code below —
        // `take_task` and `steal_task_from_other_cpu` — therefore runs under
        // that user CR3. Those routines only touch kernel-half memory, which is
        // mapped in every process page table, so that is fine *as long as the
        // page table still exists*.
        //
        // The danger is the page table being freed out from under us: if the
        // process whose CR3 we are holding exits and is reaped on another CPU,
        // its `PageTableImpl` drops and the root (PML4) / intermediate frames go
        // back to the frame allocator and get reused. The MMU then walks freed,
        // overwritten page-table memory on the next TLB miss, so our own kernel
        // stack / the iret frame we are about to build reads garbage -> `iretq`
        // to ring-0 junk -> #UD. This is exactly the intermittent SMP crash seen
        // under `apk` (rapid fork/exit) — and ctxcheck never fires because the
        // saved `UserContext` is valid; it is the physical memory behind it that
        // changes under the stale CR3.
        //
        // Hold an `Arc` to the most-recently-polled task across the next
        // `take_task`/`steal` step. That keeps its `Thread` -> `Process` ->
        // `vmar` -> page table alive, so a concurrent exit cannot free the page
        // table while its CR3 is still loaded here. The pin is replaced only
        // after the *next* poll has switched CR3 to another address space (or to
        // the kernel CR3, which `CurrentThread::drop` restores when a thread
        // finishes), so the previous page table is released only once its CR3 is
        // no longer loaded on this CPU.
        let mut _cr3_pin: Option<Arc<Task>> = None;
        loop {
            let mut task_info = self.task_collection.take_task();
            if task_info.is_none() {
                task_info = crate::runtime::steal_task_from_other_cpu();
            }
            if let Some((_key, task, waker_ref)) = task_info {
                // `waker_ref` is the task's one shared waker (built at insert):
                // no per-poll `Arc::new`, and the borrowed bit was already set
                // atomically by the generator under the collection lock —
                // setting it again here was redundant.
                let waker = woke::waker_ref(&waker_ref);
                let mut cx = Context::from_waker(&waker);
                self.task_id = task.id();
                debug!("running future {}:{}", self.id(), task.id());
                // Hang detector: a task is being polled, so clear the idle-loop
                // streak. If the machine then spins the idle loop many times with
                // tasks still present but nothing polled, a wake was lost (see the
                // else-branch dump below).
                IDLE_STREAK.store(0, core::sync::atomic::Ordering::Relaxed);
                SCHED_POLLED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                // Publish who is running before entering the future: if the
                // poll faults, the panic-containment path reads these to retire
                // exactly this task (see `runtime::abandon_current_task`). Both
                // are cleared right after the poll returns, so a fault outside
                // a poll finds no victim and the kernel halts as before.
                self.current_task = Arc::as_ptr(&task);
                self.current_waker = Arc::as_ptr(&waker_ref);
                let ret = task.poll(&mut cx);
                self.current_task = core::ptr::null();
                self.current_waker = core::ptr::null();
                // Did this future overflow the coroutine stack?  The stack is a
                // guard-page-less heap allocation: an overflow silently corrupts
                // the adjacent heap object.  Detect it immediately and panic so
                // the corrupted heap is never used — continuing with a clobbered
                // heap leads to null fn-ptr calls and an unrecoverable crash loop
                // (the labwc/Wayland crash with garbled process names).
                if !self.canary_intact() {
                    panic!(
                        "\n[stackcheck] COROUTINE STACK OVERFLOW: executor id={} task_id={} \
                         stack_base={:#x} size={:#x} — halting before corrupted heap is used\n",
                        self.id(),
                        task.id(),
                        self.stack_base,
                        STACK_SIZE
                    );
                }
                debug!("back from future {}:{}", self.id(), task.id());
                self.task_id = 0;
                // Pin this task's address space for the upcoming take_task/steal
                // (which run under the CR3 this poll just (re)loaded). Replacing
                // the previous pin here is safe: CR3 now points at *this* task's
                // page table (or at the kernel CR3 if the thread just finished —
                // `CurrentThread::drop` restored it), so the page table we drop
                // is no longer the active one. See the comment at the top of
                // `run`.
                _cr3_pin = Some(task.clone());
                // Borrow-release ordering — this is load-bearing for SMP.
                //
                // The OLD order (mark_borrowed(false), then drop_by_ref on
                // Ready) opened a window where a completed task was published
                // as (borrowed=0, dropped=0). A wake that raced with the poll
                // (deferred by take_notified while we were borrowed — routine
                // for IRQ-driven futures) then let ANOTHER executor take and
                // re-poll the SAME completed task. If a timer preemption
                // parked either executor in that window, the late
                // mark_borrowed(false) could land on a slab slot that had
                // been removed and REUSED, wiping the borrow bit of an
                // unrelated live task -> two executors polling one future ->
                // the second spins forever on the future lock
                // (task_collection.rs, Task::poll) while the first sits
                // parked as a weak executor: the >8s DEADLOCK banner.
                //
                // Therefore: on Ready, publish `dropped` FIRST and leave the
                // borrow bit SET. take_notified masks dropped tasks, so the
                // task can never be handed out again; the generator's
                // dropped-branch remove() -> clear() wipes all bits (borrow
                // included) atomically with freeing the slot, BEFORE the slot
                // can be reused by insert(). Only a Pending poll releases the
                // borrow here, and only after Task::poll has returned (future
                // lock already released).
                match ret {
                    Poll::Ready(()) => {
                        debug!("task over id = {}", task.id());
                        waker_ref.drop_by_ref();
                    }
                    Poll::Pending => {
                        waker_ref.mark_borrowed(false);
                    }
                };
                if let ExecutorState::WEAK = self.state {
                    self.state = ExecutorState::KILLED;
                    return;
                }
            } else {
                // Our run queue is drained (and stealing found nothing), so any
                // pending wake-up preemption request for this CPU has already
                // been satisfied by simply running out of work. Drop it: a stale
                // bit would suppress the coalesced IPI for the next real wake.
                crate::runtime::clear_need_resched(crate::arch::cpu_id() as usize);
                let runtime = crate::runtime::get_current_runtime();
                let task_num = runtime.task_num();
                let weak_executor = runtime.weak_executor_num();
                drop(runtime);
                // TODO: some cores may exit by mistake when we have multi-cores
                if cfg!(feature = "baremetal-test") && task_num == 0 {
                    debug!("all done! exit and reboot");
                    crate::runtime::sched_yield();
                } else if weak_executor != 0 {
                    debug!("return to runtime and run weak executor");
                    SCHED_WEAK_YIELD.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                    crate::runtime::sched_yield();
                } else if crate::runtime::run_idle_callback() {
                    // The idle callback made progress (e.g. drained deferred
                    // driver jobs that may have woken tasks): re-check the run
                    // queue instead of halting until the next interrupt.
                    continue;
                } else {
                    // Hang detector (diagnostics only): the run queue is empty so
                    // we are about to halt until the next interrupt. If tasks still
                    // exist, count idle-loop iterations; the 250 Hz timer wakes us
                    // ~every 4 ms, so ~750 iterations with no task polled ≈ 3 s.
                    // `IDLE_STREAK` is global and reset on every real poll, so it
                    // only climbs while *no* CPU makes progress.
                    //
                    // Snapshot the page bits ONLY at report cadence:
                    // `debug_pending` walks every collection under its lock,
                    // far too heavy to run on each of the ~250 idle passes/s.
                    //
                    // Classification: `notified > 0` (affinity-parked) and
                    // `borrowed > 0` (in-flight on another executor) are normal
                    // under SMP; only tasks-exist-but-nothing-pending can never
                    // recover on its own = a genuine lost wake.
                    if task_num > 0 {
                        let s = IDLE_STREAK.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                        if s == 750 || (s > 750 && s % 2500 == 0) {
                            let (tn, n, d, b) = self.task_collection.debug_pending();
                            if n == 0 && b == 0 {
                                warn!(
                                    "[sched] possible lost wake: {} task(s) parked \
                                     (notified=0 borrowed=0 dropped={}) after {} idle passes",
                                    tn, d, s
                                );
                            }
                        }
                    }
                    debug!("no other tasks, wait for interrupt");
                    // Halt protocol vs lost wakes. Publish "sleeping" FIRST,
                    // then re-check the queue with IRQs off, and only then
                    // halt (`wait_for_interrupt` is an atomic sti;hlt — an IPI
                    // arriving after the sti breaks the hlt). A remote waker
                    // does notify -> read sleeping-mask (both SeqCst): either
                    // it sees us sleeping and kicks us with the reschedule
                    // IPI, or its notify is ordered before our recheck and we
                    // see the ready bit and skip the halt. Either way the
                    // wake cannot fall into the check-then-halt window (which
                    // previously cost up to one full 4 ms tick).
                    let cpu = crate::arch::cpu_id() as usize;
                    let intr_was_on = crate::arch::intr_get();
                    crate::arch::intr_off();
                    crate::runtime::set_cpu_sleeping(cpu, true);
                    if !self.task_collection.has_ready() {
                        crate::arch::wait_for_interrupt();
                    }
                    crate::runtime::set_cpu_sleeping(cpu, false);
                    if intr_was_on {
                        crate::arch::intr_on();
                    }
                }
            }
        }
    }

    // 当前是否在运行future
    // 发生supervisor时钟中断时, 若executor在运行future, 则
    // 说明该future超时, 需要切换到另一个executor来执行其他future.
    pub fn is_running_future(&self) -> bool {
        self.task_id != 0
            || self
                .force_replace
                .load(core::sync::atomic::Ordering::Acquire)
    }

    pub fn killed(&self) -> bool {
        self.state == ExecutorState::KILLED
            || self.abandoned.load(core::sync::atomic::Ordering::SeqCst)
    }

    /// Whether this executor is *inside* `Task::poll` right now.
    ///
    /// Stricter than [`is_running_future`](Self::is_running_future), which stays
    /// true for the rest of the loop iteration after the poll returns. Only
    /// during the poll itself is there a future that can be retired, so this is
    /// what the panic-containment path tests: a fault in the scheduler's own
    /// code between polls has no task to blame and must not kill one.
    pub fn is_polling(&self) -> bool {
        !self.current_task.is_null()
    }

    /// Whether `sp` points into this executor's *usable* coroutine stack
    /// (guard bands excluded).
    ///
    /// The panic path uses this to prove that the faulting frames really are on
    /// the stack it is about to abandon, before it switches away from them. A
    /// fault taken on some other stack — the boot/idle stack, an IST stack —
    /// has nothing to do with this executor's task.
    pub fn stack_contains(&self, sp: usize) -> bool {
        (self.stack_base..self.stack_base + STACK_SIZE).contains(&sp)
    }

    /// Retire the task this executor is polling and mark the executor dead.
    ///
    /// After this, [`killed`](Self::killed) reports `true`, so the runtime drops
    /// this executor instead of ever resuming its (abandoned) stack, and the
    /// task is both marked finished and removed from the collection so no other
    /// CPU picks it up. The stack itself is not touched here — the caller is
    /// still standing on it and must switch away before it can be freed.
    ///
    /// Returns `false` when there is nothing to abandon or the future could not
    /// be retired, in which case nothing has been changed.
    ///
    /// # Safety
    ///
    /// May only be called from the CPU running this executor, as the immediate
    /// prelude to switching off its stack for good.
    pub unsafe fn abandon_current_task(&self) -> bool {
        // SAFETY: non-null means `run` is inside `Task::poll`, so the `Arc`s
        // these borrow from are live in that (current, faulted) frame.
        let Some(task) = self.current_task.as_ref() else {
            return false;
        };
        if !task.abandon() {
            return false;
        }
        // Publish the task as dropped so the collection's generator removes the
        // slab slot. The borrow bit is deliberately left set (as on the normal
        // `Ready` path) so nothing can hand this task out in the window before
        // the removal lands.
        if let Some(waker) = self.current_waker.as_ref() {
            waker.drop_by_ref();
        }
        self.abandoned
            .store(true, core::sync::atomic::Ordering::SeqCst);
        true
    }

    /// Abandon this executor after a fault taken **outside** any poll.
    ///
    /// The idle/scheduler half of [`abandon_current_task`](Self::abandon_current_task).
    /// A null-range fault on an executor's stack while it is between tasks (an
    /// IRQ landing on the idle path, a corrupted return slot in the scheduler's
    /// own frames) has NO task to retire, so `abandon_current_task` refuses and
    /// the machine used to halt — even though nothing of value was in flight and
    /// the core was perfectly recoverable.
    ///
    /// Here there is no future to leak and no task to kill: just retire the
    /// executor. `killed()` becomes true so the runtime drops it rather than
    /// resuming its corrupt stack, and `force_replace` makes the runtime build a
    /// fresh strong executor instead of switching straight back into this one.
    ///
    /// Returns `false` if a poll IS in flight — that is the task case, and
    /// killing the task is strictly better than discarding the whole executor.
    ///
    /// # Safety
    ///
    /// May only be called from the CPU running this executor, as the immediate
    /// prelude to switching off its stack for good.
    pub unsafe fn abandon_idle_executor(&self) -> bool {
        if !self.current_task.is_null() {
            return false;
        }
        self.force_replace
            .store(true, core::sync::atomic::Ordering::Release);
        self.abandoned
            .store(true, core::sync::atomic::Ordering::SeqCst);
        true
    }

    pub fn mark_weak(&mut self) {
        self.state = ExecutorState::WEAK;
    }

    pub fn id(&self) -> usize {
        self.id
    }

    pub fn task_id(&self) -> usize {
        self.task_id
    }

    /// Base address of this executor's coroutine stack (lowest address).
    pub fn stack_base(&self) -> usize {
        self.stack_base
    }

    /// [diag] Whether the soft-guard canary below the usable stack is intact.
    ///
    /// Layout: `[BOTTOM_GUARD][usable STACK_SIZE][TOP_GUARD]`. With a hard
    /// (unmapped) bottom guard, overflow #PFs before touching heap — report
    /// intact. Soft path samples canary words at both ends of the bottom guard.
    pub fn canary_intact(&self) -> bool {
        if self.hard_guard_bottom {
            return true;
        }
        unsafe {
            let p = (self.stack_base - GUARD_SIZE) as *const u64;
            // High end of the guard (first words an overflow smashes).
            let high_ok = (0..8).all(|i| {
                let idx = GUARD_WORDS - 1 - i;
                core::ptr::read_volatile(p.add(idx)) == (STACK_CANARY ^ idx as u64)
            });
            // Low end (catches a large downward jump past the edge samples).
            let low_ok =
                (0..4).all(|i| core::ptr::read_volatile(p.add(i)) == (STACK_CANARY ^ i as u64));
            high_ok && low_ok
        }
    }
}

impl Drop for Executor {
    fn drop(&mut self) {
        let alloc_base = self.stack_base - GUARD_SIZE;
        let top_guard_base = self.stack_base + STACK_SIZE;
        // Stop tracking this stack BEFORE it goes back to the heap, so a later
        // legitimate reuse of the freed range is not flagged as a double-alloc.
        stack_reg_remove(alloc_base);
        if let Some(remove) = *STACK_GUARD_REMOVE.lock() {
            if self.hard_guard_bottom {
                remove(alloc_base, GUARD_SIZE);
            }
            if self.hard_guard_top {
                remove(top_guard_base, TOP_GUARD_SIZE);
            }
        }
        unregister_stack(alloc_base);

        // Freed-stack quarantine (diagnostic; off unless STACKQUARANTINE=1).
        // Hold this stack write-protected instead of freeing it, so a dangling
        // pointer that writes into it faults at the writer (`[stack-uaf]`).
        if QUARANTINE_ENABLED.load(core::sync::atomic::Ordering::Relaxed) {
            // Never protect the stack we are standing on — a self-drop would
            // fault on our own next push. A normal executor Drop runs from
            // another stack, so this only guards against a pathological caller.
            let on_this_stack = {
                #[cfg(target_arch = "x86_64")]
                {
                    let rsp: usize;
                    // SAFETY: reads RSP only.
                    unsafe {
                        core::arch::asm!(
                            "mov {}, rsp",
                            out(reg) rsp,
                            options(nomem, nostack, preserves_flags)
                        );
                    }
                    rsp >= alloc_base && rsp < alloc_base + ALLOC_SIZE
                }
                #[cfg(not(target_arch = "x86_64"))]
                {
                    false
                }
            };
            let protect = *STACK_QUAR_PROTECT.lock();
            if !on_this_stack {
                if let Some(protect) = protect {
                    // Protect the usable region only (where the smash lands); the
                    // guard bands were just restored to normal above.
                    if protect(self.stack_base, STACK_SIZE) {
                        let evicted = quar_ring_push(alloc_base);
                        if evicted != 0 {
                            if let Some(unprotect) = *STACK_QUAR_UNPROTECT.lock() {
                                unprotect(evicted + GUARD_SIZE, STACK_SIZE);
                            }
                            // SAFETY: the evicted allocation is no longer
                            // protected and no longer referenced by the ring.
                            unsafe {
                                let s = NonNull::<u8>::new_unchecked(evicted as *mut u8);
                                Global.deallocate(s, ALLOC_LAYOUT);
                            }
                        }
                        // This stack stays quarantined; do not free it now.
                        return;
                    }
                }
            }
        }

        unsafe {
            let stack = NonNull::<u8>::new_unchecked(alloc_base as *mut u8);
            Global.deallocate(stack, ALLOC_LAYOUT);
        }
    }
}

unsafe impl Send for Executor {}
unsafe impl Sync for Executor {}

pub unsafe fn push_stack<T>(stack_top: usize, val: T) -> usize {
    let stack_top = (stack_top as *mut T).sub(1);
    *stack_top = val;
    stack_top as _
}
