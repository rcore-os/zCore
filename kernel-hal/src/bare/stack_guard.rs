//! Guard regions around the scheduler's coroutine stacks.
//!
//! A coroutine stack is a plain heap allocation (`PreemptiveScheduler`'s
//! `Executor::new`), laid out as
//! `[bottom guard][usable STACK_SIZE][top guard]`. The scheduler asks this
//! module to make those guard bands *unmapped*, so a stack that runs off either
//! end takes a clean page fault — reported by
//! [`zcore::handler`](../../../zCore/src/handler.rs) as
//! `[stack-guard] COROUTINE STACK OVERFLOW` — instead of silently overwriting
//! whichever heap object happens to sit next to it. That silent overwrite is
//! the root of the corruption hunt written up in `docs/README-crash-repro.md`:
//! sprayed return addresses, `0x87` vtables, a `memset` handed a mangled
//! pointer.
//!
//! ## State of this module
//!
//! The hard-guard (unmapping) implementation is **not present in this
//! repository** — `kernel-hal/src/bare/mod.rs` has always declared
//! `pub mod stack_guard;` but no commit ever added the file, so the tree did
//! not build. What is here restores the build with the *fallback the scheduler
//! already supports and documents*: [`install`] refuses, and `Executor::new`
//! falls back to filling the guard band with canary words
//! (`note_soft_guard_fallback`). Overflow is still detected — by the canary
//! check after every poll and by the timer-tick stack-proximity tripwire — just
//! one step later than an unmapped page would catch it.
//!
//! Restoring real hard guards means punching 4 KiB holes in the kernel mapping
//! that backs the heap (`GenericPageTable::unmap`, refusing when the range is
//! covered by a huge PTE), remembering each `(vaddr, paddr, flags)` so
//! [`remove`] can restore it before the allocation goes back to the heap, and
//! shooting down the other CPUs' TLBs. That is deliberately not guessed at
//! here: an incorrect restore corrupts the heap on every executor teardown,
//! which is the exact failure mode the guards exist to prevent.

/// Register this module's hooks with the scheduler.
///
/// Must run after the kernel page tables are pinned and *before* the first
/// `Executor::new`, which is why `primary_init` calls it right before
/// `executor::warm_runtimes()`. Boot asserts that the hooks got registered
/// (`stack_guard_hooks_registered`), so this must always register a pair, even
/// when the install side refuses every request.
pub fn init() {
    // SAFETY: the contract is that `remove` restores any mapping `install`
    // removed before the VA returns to the heap, and frees no BSS-owned frame.
    // This pair installs nothing, so both obligations hold trivially.
    unsafe { executor::set_stack_guard_hooks(install, remove) };
}

/// Try to make `[guard_base, guard_base + guard_size)` unmapped.
///
/// Returns `false` to tell the scheduler to keep its soft canary instead. See
/// the module docs: the unmapping path is not implemented in this tree.
fn install(_guard_base: usize, _guard_size: usize) -> bool {
    false
}

/// Undo an [`install`]. A no-op while `install` never takes anything away.
fn remove(_guard_base: usize, _guard_size: usize) {}

/// Whether `fault_vaddr` fell inside an installed (unmapped) stack guard.
///
/// The page-fault handler asks this first, before trying to resolve the fault
/// against the faulting thread's VMAR: a guard hit is a kernel stack overflow,
/// not anything userspace can explain. Always `false` while [`install`] refuses
/// — with the guard band mapped, a fault there cannot happen.
pub fn is_guard_fault(_fault_vaddr: usize) -> bool {
    false
}
