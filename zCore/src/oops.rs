//! Kernel fault containment: an *oops* instead of a dead machine.
//!
//! A defect in a user program — a null dereference, a divide by zero, an
//! illegal instruction — already costs only that program: `handle_user_trap`
//! turns the CPU exception into the matching Linux signal and the process dies
//! alone. What still took the whole machine down was the other direction: the
//! *kernel* faulting while serving that program. A `panic!`, an `unwrap` on an
//! argument nobody expected, a page fault in a driver path — each of those left
//! the kernel spinning in `loop { spin_loop() }`, killing every other process
//! with it, including the shell the failure could have been diagnosed from.
//!
//! This module applies the policy Linux calls an *oops*: if the fault happened
//! while serving one particular task, that task dies and the kernel lives.
//!
//! # What it costs
//!
//! Abandoning a coroutine mid-flight does **not** unwind it: not one destructor
//! of the aborted call chain runs, so its heap allocations, its `Arc`
//! references and its 2 MiB stack are all lost. That is bounded leakage per
//! contained fault, which is why there is a budget ([`MAX_CONTAINED`]): a
//! kernel that spends its day containing faults is not healthy, and past that
//! point a halt somebody can diagnose beats silent degradation.
//!
//! # When it declines
//!
//! Recovery is only safe if the fault happened somewhere the kernel can be left
//! from without leaving it half-updated. Containment is refused — and the
//! machine halts, as before — when:
//!
//! - the CPU holds any kernel lock (`lock::lock_depth() != 0`): it was in the
//!   middle of mutating shared state, and that lock would never be released;
//! - the fault arrived in interrupt context (timer callback): there is no task
//!   to attribute it to, and the interrupted work is not the sufferer's;
//! - no coroutine was running, or the faulting stack is not its own, or its
//!   overflow canary is already clobbered — with the heap smashed, continuing
//!   only spreads the damage;
//! - this same CPU was already containing another fault: if the recovery
//!   machinery is what broke, there is nothing left to rescue;
//! - the fault budget is exhausted.
//!
//! These conditions are necessary, not sufficient: a handful of scheduler and
//! console locks are external `spin::Mutex`es that do not count towards
//! `lock_depth`. The case that matters — the kernel objects (process, thread,
//! VMAR, files) — does use `lock::Mutex` and so is covered.
//!
//! `PANICONOOPS=1` on the kernel command line turns all of this off and
//! restores the previous behaviour (halt on the first fault), which is what one
//! wants while debugging a specific failure: the machine freezes on the spot.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use kernel_hal::console::{serial_write_fmt_spin, serial_write_str};
use zircon_object::object::KernelObject;
use zircon_object::task::Thread;

/// How many faults to contain before giving up and halting.
///
/// Not a resource limit (each leak is a couple of MiB out of a heap of
/// hundreds) but a credibility one: past this count the kernel is failing
/// systematically, and what helps then is a halt with its dump, not more
/// running.
const MAX_CONTAINED: u32 = 16;

/// Exit code for the victim: the same `128 + SIGKILL` `sys_kill` uses, so the
/// parent's `wait4` — and the shell's `$?` — read it as what it is, a process
/// the system put down.
const KILLED_BY_KERNEL: i64 = 128 + 9;

/// Faults contained since boot.
static CONTAINED: AtomicU32 = AtomicU32::new(0);

/// `PANICONOOPS=1`: contain nothing, halt as before.
static PANIC_ON_OOPS: AtomicBool = AtomicBool::new(false);

/// Mask of CPUs currently containing a fault, so a fault *inside* the recovery
/// path is recognised as such. One bit per logical CPU; the system's maximum
/// (`lock::MAX_CORE_NUM` = 64) is exactly what fits here.
static CONTAINING: AtomicU64 = AtomicU64::new(0);

/// Enable/disable halting on the first fault (`PANICONOOPS=1`).
pub fn set_panic_on_oops(enabled: bool) {
    PANIC_ON_OOPS.store(enabled, Ordering::SeqCst);
}

/// How many kernel faults have been contained since boot.
pub fn contained_count() -> u32 {
    CONTAINED.load(Ordering::Relaxed)
}

/// Try to contain the fault `what` by killing only whoever caused it.
///
/// **Does not return** if it succeeds: the CPU goes back to the scheduler and
/// keeps running the rest of the system. If it returns, containment was
/// declined and the caller must halt the machine as it did before; the reason
/// is already on the console.
///
/// `what` is the short label of the fault (`"kernel panic"`,
/// `"null-range #PF"`) that appears in the report.
///
/// `restore_kd` is the console mode the fault path forced to text so its dump
/// would be visible. Containing means putting it back, or a live compositor
/// would be left rendering into a buffer that is no longer presented and the
/// desktop would look frozen. `None` if the caller did not touch it.
///
/// Must be called with interrupts already disabled, from the fault path, having
/// touched nothing on the way.
pub fn try_contain(what: &str, restore_kd: Option<u32>) {
    if PANIC_ON_OOPS.load(Ordering::Relaxed) {
        serial_write_fmt_spin(format_args!(
            "\n[oops] {} NOT contained: PANICONOOPS=1 — halting\n",
            what
        ));
        return;
    }

    let cpu = kernel_hal::cpu::cpu_id() as usize;
    if cpu >= 64 {
        serial_write_str("\n[oops] cpu id out of range — halting\n");
        return;
    }
    let bit = 1u64 << cpu;
    if CONTAINING.fetch_or(bit, Ordering::SeqCst) & bit != 0 {
        // Recovery itself faulted. No `format_args!` — if what broke was the
        // heap, formatting is exactly what faults again right here.
        serial_write_str("\n[oops] fault WHILE containing a fault — halting\n");
        return;
    }

    // Every refusal states its reason: that is the difference between "the
    // machine halted" and "the machine halted because the fault arrived with
    // two locks held", which is what says where to look next.
    let decline = |reason: core::fmt::Arguments| {
        serial_write_fmt_spin(format_args!(
            "\n[oops] {} NOT contained ({}) — halting\n",
            what, reason
        ));
        CONTAINING.fetch_and(!bit, Ordering::SeqCst);
    };

    let depth = lock::lock_depth();
    if depth != 0 {
        return decline(format_args!("the CPU holds {} kernel lock(s)", depth));
    }
    if kernel_hal::timer::in_timer_callback() {
        return decline(format_args!(
            "fault in the timer callback, not attributable to a process"
        ));
    }
    if executor::heap_smash_suspected() {
        return decline(format_args!("the heap was already suspected of a smash"));
    }
    if !executor::current_task_abandonable() {
        return decline(format_args!(
            "no abandonable coroutine was running on this CPU"
        ));
    }

    let n = CONTAINED.fetch_add(1, Ordering::SeqCst) + 1;
    if n > MAX_CONTAINED {
        return decline(format_args!(
            "budget exhausted ({} faults already contained)",
            MAX_CONTAINED
        ));
    }

    // Containing from here. First name the victim — there may be none, if the
    // coroutine belongs to the kernel itself rather than to a user thread.
    let victim = kernel_hal::thread::get_current_thread()
        .and_then(|thread| thread.downcast::<Thread>().ok());

    match &victim {
        Some(thread) => {
            let report = format_args!(
                "\n[oops] {} contained ({}/{}): killing pid={} {:?} tid={} — \
                 the kernel stays up, no other process is affected\n",
                what,
                n,
                MAX_CONTAINED,
                thread.proc().id(),
                thread.proc().name(),
                thread.id(),
            );
            serial_write_fmt_spin(report);
            // To the monitor too: on a box with no serial cable the red panic
            // banner just painted is all anyone sees, and without this line it
            // would read as a dead system when in fact it is still running.
            kernel_hal::console::graphic_console_write_fmt_spin(report);
            kill(thread);
        }
        None => {
            // No process to blame: the coroutine was the kernel's own (net
            // polling, a driver's deferred work...). It is retired anyway,
            // because halting the machine is strictly worse — but say so
            // loudly, since the symptom will be a subsystem that quietly stops
            // responding with nobody dead to explain it.
            serial_write_fmt_spin(format_args!(
                "\n[oops] {} contained ({}/{}): the coroutine was the KERNEL's, not a \
                 process's — retiring it; the subsystem it served may stay dead until \
                 the next boot\n",
                what, n, MAX_CONTAINED,
            ));
        }
    }

    // Give the console back to whoever had it. The fault path forced it to text
    // so its dump would show; if the system is going to keep running, the
    // compositor has to own the screen again on its next frame.
    if let Some(mode) = restore_kd {
        kernel_hal::console::set_kd_mode(mode);
    }

    // The delicate part is done and only the context switch is left, so release
    // the re-entrancy guard: a later fault on this CPU should get its own
    // chance to be contained.
    CONTAINING.fetch_and(!bit, Ordering::SeqCst);

    // SAFETY: we are on the fault path, standing on the coroutine's own stack
    // (proven by `current_task_abandonable`), with interrupts disabled and no
    // kernel lock held (checked above). Nothing from the abandoned call chain
    // is touched again.
    if unsafe { executor::abandon_current_task() } {
        unreachable!("abandon_current_task returned after containing the fault");
    }
    serial_write_str("\n[oops] could not abandon the coroutine — halting\n");
}

/// Kill the process owning the faulted thread and take the thread off its list.
fn kill(thread: &alloc::sync::Arc<Thread>) {
    // Same as `CurrentThread::drop`: leave the dying process's page table
    // *before* its address space is torn down, so we are not left executing on
    // a CR3 whose tables are being freed.
    kernel_hal::vm::activate_kernel_paging();
    // This CPU's current thread points at the one just given up for dead. The
    // next coroutine to run here will set its own, but until then nobody should
    // find it.
    kernel_hal::thread::set_current_thread(None);

    let proc = thread.proc();
    // The same path a `kill -9` takes: publish the exit status, wake the
    // parent's `wait4`, and tell the process's other threads to die.
    proc.exit(KILLED_BY_KERNEL);
    // This thread's `CurrentThread` leaked with the abandoned coroutine, so its
    // `Drop` will never take it off the process's thread list. Doing it here is
    // what lets the process actually terminate and release its address space
    // when this was its last thread.
    thread.terminate_abandoned();
}
