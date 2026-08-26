//! [diag] Hardware data watchpoints (x86 debug registers DR0–DR3).
//!
//! Built for one specific job: catching the wild write documented in
//! `docs/README-crash-repro.md` — the one that sprays small garbage values
//! (`0x01`, `0x0a`, `0x87`) over live kernel pointers, mangling saved return
//! addresses on the executor stack and `Arc`/vtable words on the heap.
//!
//! That hunt ruled watchpoints out, and rightly so *for its chosen target*:
//! the corrupted slot was a saved return address in `run_executor`'s frame —
//! a slot the kernel legitimately writes on every call and `ret`, so a
//! write-watch there fires constantly and cannot single out the wild write
//! (and debug registers cannot match on a *value*).
//!
//! The reasoning does not carry to a **write-once** victim. A `KObjectBase`
//! name is a `String` built when the object is created and then only read;
//! a page-fault report showing a process name as `"l\u{fffd}"` means that
//! buffer was overwritten by something that had no business touching it. A
//! write-watch on those bytes is silent under normal operation, so the first
//! trap it takes is the corruptor itself — with its RIP in the trap frame.
//! That is the datum the whole investigation has been missing.
//!
//! Mechanics, and why it is shaped this way:
//! - Debug registers are **per-CPU** and are not part of the task context, so
//!   arming DR0 on the CPU that happens to call [`watch_write`] would leave
//!   every other core unwatched — and the corruptor is as likely to run there.
//!   Instead the request is published globally with a generation counter, and
//!   each CPU programs its own registers from the timer tick ([`sync_this_cpu`],
//!   called next to the executor canary check), so the watch is live on every
//!   core within one tick (~4 ms).
//! - A data watchpoint is a **trap**, not a fault: it is delivered *after* the
//!   store retires, so the handler reports and resumes. The write still lands
//!   — the point is to name the writer, not to prevent the corruption.
//! - DR7 is programmed with the *local* enable bit only; DR6's sticky match
//!   bits are cleared on the way out, as the architecture requires (the CPU
//!   never clears them itself, and a stale bit makes the next trap
//!   unattributable).
//!
//! Costs nothing when idle: no watchpoint means DR7 stays 0 and the per-tick
//! sync is a single relaxed load and compare.

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};

use crate::config::MAX_CORE_NUM;

/// Watched address (0 = no watchpoint armed).
static WP_ADDR: AtomicU64 = AtomicU64::new(0);
/// Watched span in bytes: 1, 2, 4 or 8.
static WP_LEN: AtomicUsize = AtomicUsize::new(0);
/// Bumped on every arm/disarm so each CPU notices it must reprogram.
static WP_GEN: AtomicU64 = AtomicU64::new(0);
/// Generation each CPU has already written into its own debug registers.
static WP_CPU_GEN: [AtomicU64; MAX_CORE_NUM] = [const { AtomicU64::new(0) }; MAX_CORE_NUM];
/// Executor spine-slot generation each CPU last programmed (see below).
static SPINE_CPU_GEN: [AtomicU64; MAX_CORE_NUM] = [const { AtomicU64::new(0) }; MAX_CORE_NUM];
/// How many times the watchpoint has fired (reported in the trap log).
static WP_HITS: AtomicU64 = AtomicU64::new(0);

// ── Executor spine-slot auto-watch (null-exec hunt, generation 2) ────────────
//
// The executor crate publishes each live executor's *spine slot* — the
// write-once `return-into-run_executor` qword at `stack_top - 0x508` that
// every `[null-exec]` capture shows zeroed (see the registry doc in
// `PreemptiveScheduler/src/executor.rs`). Nothing may legitimately store to a
// REGISTERED slot, so this module keeps DR0–DR3 loaded with the first four
// registered slots on every CPU: the corruptor's store traps on the CPU that
// executes it, with the writer's rip in the trap frame — the datum six
// post-mortem captures could not provide. A manual [`watch_write`] keeps
// priority on DR0; spine slots fill the remaining registers.
//
// Noise filtering is structural, not heuristic: registration brackets the
// write-once window (armed only between `Executor::run` entry and its
// return/Drop), and a hit whose address is no longer registered — the stack
// was retired and re-poisoned between our last tick sync and the store — is
// dropped silently.

/// What this CPU currently holds in DR0..DR3 (0 = disabled), for `#DB`
/// attribution. Written only by the owning CPU's `sync`.
static DR_ARMED: [[AtomicU64; 4]; MAX_CORE_NUM] =
    [const { [const { AtomicU64::new(0) }; 4] }; MAX_CORE_NUM];
/// Full spine-writer reports so far; arming stops after a few so a pathological
/// hot slot cannot storm the console.
static SPINE_TRAP_HITS: AtomicU64 = AtomicU64::new(0);
/// Latched off after enough reports (checked by `sync`).
static SPINE_WATCH_OFF: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Arm a write watchpoint covering `len` bytes at `addr` on **every** CPU.
///
/// `len` must be 1, 2, 4 or 8 and `addr` must be `len`-aligned — x86 ignores
/// misaligned watchpoints rather than reporting an error, so a bad request is
/// rejected here instead of silently never firing.
///
/// Only one watchpoint (DR0) is used: the callers that matter arm one target
/// and want it live everywhere, and keeping DR1–DR3 free leaves room for a
/// second probe without reworking this.
pub fn watch_write(addr: usize, len: usize) -> bool {
    if addr == 0 || !matches!(len, 1 | 2 | 4 | 8) || addr % len != 0 {
        return false;
    }
    WP_ADDR.store(addr as u64, Relaxed);
    WP_LEN.store(len, Relaxed);
    WP_GEN.fetch_add(1, Relaxed);
    true
}

/// Disarm the watchpoint on every CPU.
pub fn clear_watch() {
    WP_ADDR.store(0, Relaxed);
    WP_LEN.store(0, Relaxed);
    WP_GEN.fetch_add(1, Relaxed);
}

/// The currently armed (address, len), or None.
pub fn armed() -> Option<(usize, usize)> {
    let a = WP_ADDR.load(Relaxed) as usize;
    (a != 0).then(|| (a, WP_LEN.load(Relaxed)))
}

/// Number of times the watchpoint has fired since boot.
pub fn hits() -> u64 {
    WP_HITS.load(Relaxed)
}

// ── x86_64 bare implementation ───────────────────────────────────────────────

#[cfg(all(target_arch = "x86_64", target_os = "none"))]
mod imp {
    use super::*;
    use core::arch::asm;

    /// DR7: local exact-breakpoint reporting (ignored by modern CPUs, but the
    /// architecture manual still recommends setting it).
    const DR7_LE: u64 = 1 << 8;

    /// DR7 local-enable bit for slot `i` (bits 0, 2, 4, 6).
    fn l_bit(i: usize) -> u64 {
        1 << (i * 2)
    }

    /// DR7 R/W field for slot `i`: 0b01 = break on data writes only.
    fn rw_write(i: usize) -> u64 {
        0b01 << (16 + i * 4)
    }

    /// DR7 LEN field for slot `i`. The encoding is *not* ordinal: 8 bytes is
    /// 0b10, which sorts between 2 and 4 bytes.
    fn len_bits(i: usize, len: usize) -> u64 {
        let code: u64 = match len {
            1 => 0b00,
            2 => 0b01,
            8 => 0b10,
            _ => 0b11, // 4
        };
        code << (18 + i * 4)
    }

    /// SAFETY (all helpers): `mov` to/from a debug register is valid at
    /// CPL 0, which is where every caller runs (timer tick, trap handler).
    /// Debug registers hold no state the compiler tracks, so these are opaque
    /// side-effecting instructions to it.
    unsafe fn write_dr(i: usize, v: u64) {
        match i {
            0 => asm!("mov dr0, {}", in(reg) v, options(nostack, preserves_flags)),
            1 => asm!("mov dr1, {}", in(reg) v, options(nostack, preserves_flags)),
            2 => asm!("mov dr2, {}", in(reg) v, options(nostack, preserves_flags)),
            _ => asm!("mov dr3, {}", in(reg) v, options(nostack, preserves_flags)),
        }
    }

    unsafe fn write_dr7(v: u64) {
        asm!("mov dr7, {}", in(reg) v, options(nostack, preserves_flags));
    }

    unsafe fn read_dr6() -> u64 {
        let v: u64;
        asm!("mov {}, dr6", out(reg) v, options(nostack, preserves_flags));
        v
    }

    unsafe fn write_dr6(v: u64) {
        asm!("mov dr6, {}", in(reg) v, options(nostack, preserves_flags));
    }

    /// Bring this CPU's debug registers in line with the requested manual
    /// watchpoint (DR0 priority) plus the executor spine slots (remaining
    /// registers). Re-programs only when either generation moved.
    pub(super) fn sync(cpu: usize) {
        let want_wp = WP_GEN.load(Relaxed);
        let want_spine = if SPINE_WATCH_OFF.load(Relaxed) {
            u64::MAX // latched off: stable sentinel so we reprogram once
        } else {
            ::executor::spine_gen()
        };
        if cpu >= MAX_CORE_NUM
            || (WP_CPU_GEN[cpu].load(Relaxed) == want_wp
                && SPINE_CPU_GEN[cpu].load(Relaxed) == want_spine)
        {
            return;
        }

        // Compose the desired four (addr, len) pairs.
        let manual = WP_ADDR.load(Relaxed);
        let manual_len = WP_LEN.load(Relaxed);
        let mut desired: [(u64, usize); 4] = [(0, 8); 4];
        let mut next = 0usize;
        if manual != 0 {
            desired[0] = (manual, manual_len);
            next = 1;
        }
        if !SPINE_WATCH_OFF.load(Relaxed) {
            let mut slots = [0usize; 4];
            let n = ::executor::spine_snapshot(&mut slots);
            for &s in slots[..n].iter() {
                if next >= 4 {
                    break;
                }
                if s != 0 && s as u64 != manual {
                    desired[next] = (s as u64, 8);
                    next += 1;
                }
            }
        }

        // Program: disable everything first so no CPU ever runs with DR7
        // enabled against a half-written DRn, then load and enable.
        let mut dr7 = 0u64;
        unsafe {
            write_dr7(0);
            for (i, &(addr, len)) in desired.iter().enumerate() {
                write_dr(i, addr);
                DR_ARMED[cpu][i].store(addr, Relaxed);
                if addr != 0 {
                    dr7 |= l_bit(i) | rw_write(i) | len_bits(i, len);
                }
            }
            if dr7 != 0 {
                write_dr7(dr7 | DR7_LE);
            }
        }
        WP_CPU_GEN[cpu].store(want_wp, Relaxed);
        SPINE_CPU_GEN[cpu].store(want_spine, Relaxed);
    }

    /// Frame-pointer backtrace of the writer — the caller chain names the code
    /// path, not just the storing instruction (often an inlined memcpy).
    fn report_writer_chain(tag: &str, rbp: u64) {
        let plausible = |a: u64| a >= 0xffff_ff00_0000_0000 && a < 0xffff_ff00_1000_0000;
        let mut fp = rbp;
        let mut i = 0usize;
        while i < 12 && plausible(fp) && (fp & 0x7) == 0 {
            // SAFETY: fp is a validated, 8-aligned kernel-range address; a
            // frame is [saved_rbp, return_addr].
            let saved = unsafe { core::ptr::read_volatile(fp as *const u64) };
            let ret = unsafe { core::ptr::read_volatile((fp + 8) as *const u64) };
            crate::console::serial_write_fmt_spin(format_args!("[{tag}]   #{i:02} ret={ret:#x}\n"));
            if saved <= fp {
                break; // frame pointers must strictly increase up the stack
            }
            fp = saved;
            i += 1;
        }
    }

    /// Handle a #DB. Returns true when it was one of our watchpoints (and
    /// therefore already reported and cleared), false for any other debug
    /// exception — a real breakpoint, single-step — which the caller must
    /// handle as before.
    pub(super) fn handle_debug(rip: u64, rsp: u64, rbp: u64, cpu: usize) -> bool {
        // DR6 bits 0-3 = DRn matched. The bits are sticky: the CPU sets them
        // and never clears them, so a stale bit would misattribute the next
        // #DB. Clear the ones we consume.
        let dr6 = unsafe { read_dr6() };
        let matched = dr6 & 0b1111;
        if matched == 0 {
            return false;
        }
        unsafe { write_dr6(dr6 & !0b1111) };
        if cpu >= MAX_CORE_NUM {
            return true;
        }

        let manual = WP_ADDR.load(Relaxed);
        for slot in 0..4usize {
            if matched & (1 << slot) == 0 {
                continue;
            }
            let addr = DR_ARMED[cpu][slot].load(Relaxed);
            if addr == 0 {
                continue; // stale DR6 residue for a slot we already disabled
            }

            if manual != 0 && addr == manual {
                // ── Manual watch (legacy single-target path) ────────────────
                let n = WP_HITS.fetch_add(1, Relaxed) + 1;
                let len = WP_LEN.load(Relaxed);
                // Spin/blocking serial writer: this must survive even if the
                // machine is already in the corrupted state that motivated
                // the watch.
                crate::console::serial_write_fmt_spin(format_args!(
                    "\n[watchpoint] HIT #{n} on {len}B at {addr:#x} — WRITTEN BY rip={rip:#x} \
                     (cpu{cpu} rsp={rsp:#x} rbp={rbp:#x})\n\
                     [watchpoint] symbolize with: llvm-addr2line -e <zcore.elf> -fC {rip:#x}\n",
                ));
                report_writer_chain("watchpoint", rbp);
                // Bounded catch: a handful of hits is enough to name the
                // writer's rip. Disarm after that so a hot word cannot storm
                // the console.
                if n >= 6 {
                    super::clear_watch();
                    crate::console::serial_write_str(
                        "[watchpoint] disarming after 6 hits — writer rip(s) captured above\n",
                    );
                }
                continue;
            }

            // ── Executor spine slot ─────────────────────────────────────────
            // A hit whose slot is no longer registered is pool-retire noise
            // (the stack was dropped and re-poisoned between our last tick
            // sync and this store): drop it silently.
            let Some(exec_id) = ::executor::spine_owner_of(addr as usize) else {
                continue;
            };
            // The trap is delivered AFTER the store retires, so the slot now
            // holds the corruptor's value.
            // SAFETY: registered slots stay mapped (executor stacks are
            // pooled, never unmapped).
            let new_val = unsafe { core::ptr::read_volatile(addr as *const u64) };
            let n = SPINE_TRAP_HITS.fetch_add(1, Relaxed) + 1;
            crate::console::serial_write_fmt_spin(format_args!(
                "\n[spine-writer] HIT #{n}: executor id={exec_id} spine slot {addr:#x} \
                 OVERWRITTEN with {new_val:#x}\n\
                 [spine-writer] writer: cpu{cpu} rip={rip:#x} rsp={rsp:#x} rbp={rbp:#x}\n\
                 [spine-writer] symbolize with: llvm-addr2line -e <zcore.elf> -fCi {rip:#x}\n",
            ));
            report_writer_chain("spine-writer", rbp);
            // The writer's locals name its buffer: dump a window around its
            // stack pointer (bounded, kernel-range guarded).
            let lo = rsp & !0x7;
            for k in 0..24u64 {
                let a = lo + k * 8;
                if a < 0xffff_ff00_0000_0000 || a >= 0xffff_ff01_0000_0000 {
                    break;
                }
                let v = unsafe { core::ptr::read_volatile(a as *const u64) };
                crate::console::serial_write_fmt_spin(format_args!(
                    "[spine-writer]   wrsp+{:#04x} @{a:#x} = {v:#018x}\n",
                    k * 8,
                ));
            }
            ::executor::note_heap_smash_suspected();
            // A few catches are enough; stop arming spine slots afterwards so
            // an unexpected legitimate writer cannot storm the console.
            if n >= 4 {
                SPINE_WATCH_OFF.store(true, Relaxed);
                crate::console::serial_write_str(
                    "[spine-writer] disarming spine watch after 4 hits — rip(s) captured above\n",
                );
            }
        }
        true
    }
}

/// Program this CPU's debug registers if the requested watchpoint changed.
/// Called from the timer tick, so every core picks up an arm/disarm within
/// one tick without an IPI.
#[cfg(all(target_arch = "x86_64", target_os = "none"))]
pub fn sync_this_cpu() {
    imp::sync(crate::cpu::cpu_id() as usize);
}

/// Handle a debug exception; see `imp::handle_debug`.
#[cfg(all(target_arch = "x86_64", target_os = "none"))]
pub fn handle_debug_trap(rip: u64, rsp: u64, rbp: u64) -> bool {
    imp::handle_debug(rip, rsp, rbp, crate::cpu::cpu_id() as usize)
}

// ── Stubs for every other target (libos, riscv, aarch64) ─────────────────────

#[cfg(not(all(target_arch = "x86_64", target_os = "none")))]
pub fn sync_this_cpu() {}

#[cfg(not(all(target_arch = "x86_64", target_os = "none")))]
pub fn handle_debug_trap(_rip: u64, _rsp: u64, _rbp: u64) -> bool {
    false
}
