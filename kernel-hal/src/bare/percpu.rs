//! Per-CPU storage block.
//!
//! Modeled on Redox OS's `PercpuBlock` / `ProcessorControlRegion`: each CPU owns
//! a single [`PercpuBlock`] that consolidates its per-CPU state, instead of
//! scattering separate `[_; MAX_CORE_NUM]` arrays indexed by CPU id.
//!
//! The current CPU's block is reached through [`current`]. On x86_64 the block
//! pointer lives in the GS region set up by `trapframe` (read with a single
//! `mov reg, gs:[off]`, no array indexing) — the same trick Redox uses with its
//! PCR. On architectures whose per-CPU register fast-path is not wired up yet,
//! [`current`] falls back to indexing [`PERCPU`] by the dense logical CPU id,
//! which is bounded and therefore safe.

use alloc::sync::Arc;
use core::any::Any;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::config::MAX_CORE_NUM;
use crate::utils::PerCpuCell;

/// Consolidated per-CPU state.
pub struct PercpuBlock {
    /// Dense logical CPU id that owns this block (`u32::MAX` until registered).
    cpu_id: AtomicU32,
    /// The thread currently running on this CPU.
    pub current_thread: PerCpuCell<Option<Arc<dyn Any + Send + Sync>>>,
    /// Remaining timer ticks before the current task must yield.
    /// See [`tick_should_preempt`].
    tick_quantum: PerCpuCell<u32>,
    /// Whether this CPU's LAPIC timer has been stretched for tickless idle
    /// (see `timer::timer_idle_enter`). Touched only by its owning CPU.
    timer_idle_armed: PerCpuCell<bool>,
    /// Nesting depth of `timer_tick` on this CPU (housekeeping + Box
    /// callbacks). A counter (not a bool) because work can re-enable IRQs via
    /// lock `pop_off` and re-enter `timer_tick`. Lets the #PF path distinguish
    /// a corrupt fn-ptr/vtable in the timer path from a userspace fault that
    /// merely interrupted the same process.
    timer_callback_depth: PerCpuCell<u32>,
}

impl PercpuBlock {
    const fn new() -> Self {
        Self {
            cpu_id: AtomicU32::new(u32::MAX),
            current_thread: PerCpuCell::new(None),
            tick_quantum: PerCpuCell::new(0),
            timer_idle_armed: PerCpuCell::new(false),
            timer_callback_depth: PerCpuCell::new(0),
        }
    }

    /// The dense logical id of the CPU this block belongs to.
    #[inline]
    pub fn cpu_id(&self) -> u32 {
        self.cpu_id.load(Ordering::Relaxed)
    }
}

/// How many timer ticks one task gets before the preemption point fires.
///
/// At 250 Hz (4 ms tick), 5 ticks ≈ 20 ms — close to Linux's default
/// non-interactive time slice. Yielding on every raw tick (the previous
/// behaviour) churned the async executor at 250 Hz even when the same task
/// was the only runnable one, which showed up as scheduler overhead on
/// CPU-bound workloads.
const TICKS_PER_QUANTUM: u32 = 5;

/// Decrement the current CPU's quantum and report whether the caller should
/// yield. Returns `true` exactly once every [`TICKS_PER_QUANTUM`] calls from a
/// given CPU.
///
/// Called from the timer-interrupt path of the user-trap handler; the cell is
/// only ever touched by its owning CPU, so the unsynchronised access is sound.
#[inline]
pub fn tick_should_preempt() -> bool {
    let cell = &current().tick_quantum;
    let n = *cell.get();
    if n == 0 {
        *cell.get_mut() = TICKS_PER_QUANTUM - 1;
        true
    } else {
        *cell.get_mut() = n - 1;
        false
    }
}

/// Whether the current CPU's LAPIC timer is currently stretched for tickless
/// idle. Only ever read/written by the owning CPU.
#[inline]
pub fn timer_idle_armed() -> bool {
    *current().timer_idle_armed.get()
}

/// Record whether the current CPU's LAPIC timer is stretched for tickless idle.
#[inline]
pub fn set_timer_idle_armed(armed: bool) {
    *current().timer_idle_armed.get_mut() = armed;
}

/// Whether this CPU is currently inside `timer_tick` (any stage).
#[inline]
pub fn in_timer_callback() -> bool {
    *current().timer_callback_depth.get() > 0
}

/// Enter `timer_tick` on this CPU (nesting-safe).
#[inline]
pub fn begin_timer_callback() {
    let cell = &current().timer_callback_depth;
    *cell.get_mut() = cell.get().saturating_add(1);
}

/// Leave `timer_tick` on this CPU (nesting-safe).
#[inline]
pub fn end_timer_callback() {
    let cell = &current().timer_callback_depth;
    *cell.get_mut() = cell.get().saturating_sub(1);
}

/// Backing storage for every CPU's block, indexed by dense logical CPU id.
///
/// Used both as cross-CPU storage and as the fallback for [`current`] before the
/// per-CPU register fast-path is established.
static PERCPU: [PercpuBlock; MAX_CORE_NUM] = [const { PercpuBlock::new() }; MAX_CORE_NUM];

/// Architecture fast-path: pointer to the current CPU's block, or null if not
/// yet established on this CPU / arch.
#[inline]
fn arch_percpu_ptr() -> *const PercpuBlock {
    #[cfg(target_arch = "x86_64")]
    {
        trapframe::read_cpu_local() as *const PercpuBlock
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        core::ptr::null()
    }
}

/// Record this CPU's block pointer in its per-CPU register, if supported.
#[inline]
fn set_arch_percpu_ptr(_block: &'static PercpuBlock) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        // Safe: `trapframe::init()` has run on this CPU before `register`.
        trapframe::write_cpu_local(_block as *const PercpuBlock as usize);
    }
}

/// The current CPU's [`PercpuBlock`].
#[inline]
pub fn current() -> &'static PercpuBlock {
    let ptr = arch_percpu_ptr();
    if ptr.is_null() {
        // Fallback before the register fast-path is set (or on arches without
        // one). `cpu_id()` is the dense logical id; bound-check defensively.
        let id = crate::cpu::cpu_id() as usize;
        PERCPU.get(id).unwrap_or(&PERCPU[0])
    } else {
        unsafe { &*ptr }
    }
}

/// Bind the current CPU to its [`PercpuBlock`].
///
/// Call once per CPU, after `trapframe::init()` (which sets up the GS region on
/// x86_64) and after the CPU's logical id is known.
pub fn register() {
    // Establish this CPU's hardware-id -> logical-id mapping where the arch needs
    // to self-assign (riscv). On x86_64 the mapping is set during SMP enumeration.
    #[cfg(any(target_arch = "riscv64", target_arch = "aarch64"))]
    {
        crate::cpu::register_logical_id();
    }
    let id = crate::cpu::cpu_id() as usize;
    if let Some(block) = PERCPU.get(id) {
        block.cpu_id.store(id as u32, Ordering::Relaxed);
        #[cfg(target_arch = "x86_64")]
        unsafe {
            trapframe::write_logical_cpu_id(id as u8);
        }
        set_arch_percpu_ptr(block);
    }
}
