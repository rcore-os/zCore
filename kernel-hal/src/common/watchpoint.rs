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
/// How many times the watchpoint has fired (reported in the trap log).
static WP_HITS: AtomicU64 = AtomicU64::new(0);

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

    /// DR7: local-enable for DR0.
    const DR7_L0: u64 = 1 << 0;
    /// DR7: local exact-breakpoint reporting (ignored by modern CPUs, but the
    /// architecture manual still recommends setting it).
    const DR7_LE: u64 = 1 << 8;
    /// DR7 bits 16–17: break condition for DR0. 0b01 = data writes only.
    const DR7_RW0_WRITE: u64 = 0b01 << 16;

    /// DR7 bits 18–19: watched span for DR0. The encoding is *not* ordinal:
    /// 8 bytes is 0b10, which sorts between 2 and 4 bytes.
    fn len_bits(len: usize) -> u64 {
        let code: u64 = match len {
            1 => 0b00,
            2 => 0b01,
            8 => 0b10,
            _ => 0b11, // 4
        };
        code << 18
    }

    /// SAFETY (all four helpers): `mov` to/from a debug register is valid at
    /// CPL 0, which is where every caller runs (timer tick, trap handler).
    /// Debug registers hold no state the compiler tracks, so these are opaque
    /// side-effecting instructions to it.
    unsafe fn write_dr0(v: u64) {
        asm!("mov dr0, {}", in(reg) v, options(nostack, preserves_flags));
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

    /// Bring this CPU's debug registers in line with the requested watchpoint.
    pub(super) fn sync(cpu: usize) {
        let want = WP_GEN.load(Relaxed);
        if cpu >= MAX_CORE_NUM || WP_CPU_GEN[cpu].load(Relaxed) == want {
            return;
        }
        let addr = WP_ADDR.load(Relaxed);
        let len = WP_LEN.load(Relaxed);
        unsafe {
            if addr == 0 {
                // Disarm first, then clear the address: a CPU must never see
                // DR7 enabled against a half-written DR0.
                write_dr7(0);
                write_dr0(0);
            } else {
                write_dr7(0);
                write_dr0(addr);
                write_dr7(DR7_L0 | DR7_LE | DR7_RW0_WRITE | len_bits(len));
            }
        }
        WP_CPU_GEN[cpu].store(want, Relaxed);
    }

    /// Handle a #DB. Returns true when it was our watchpoint (and therefore
    /// already reported and cleared), false for any other debug exception —
    /// a real breakpoint, single-step — which the caller must handle as before.
    pub(super) fn handle_debug(rip: u64, rsp: u64, rbp: u64, cpu: usize) -> bool {
        // DR6 bit 0 (B0) = DR0 matched. The bits are sticky: the CPU sets them
        // and never clears them, so a stale bit would misattribute the next
        // #DB. Clear the ones we consume.
        let dr6 = unsafe { read_dr6() };
        if dr6 & 1 == 0 {
            return false;
        }
        unsafe { write_dr6(dr6 & !0b1111) };
        let n = WP_HITS.fetch_add(1, Relaxed) + 1;
        let (addr, len) = (WP_ADDR.load(Relaxed), WP_LEN.load(Relaxed));

        // Spin/blocking serial writer: this must survive even if the machine
        // is already in the corrupted state that motivated the watch.
        crate::console::serial_write_fmt_spin(format_args!(
            "\n[watchpoint] HIT #{n} on {len}B at {addr:#x} — WRITTEN BY rip={rip:#x} \
             (cpu{cpu} rsp={rsp:#x} rbp={rbp:#x})\n\
             [watchpoint] symbolize with: llvm-addr2line -e <zcore.elf> -fC {rip:#x}\n",
        ));
        // The caller chain names the code path, not just the storing
        // instruction (which is often an inlined memcpy).
        let plausible = |a: u64| a >= 0xffff_ff00_0000_0000 && a < 0xffff_ff00_1000_0000;
        let mut fp = rbp;
        let mut i = 0usize;
        while i < 12 && plausible(fp) && (fp & 0x7) == 0 {
            // SAFETY: fp is a validated, 8-aligned kernel-range address; a
            // frame is [saved_rbp, return_addr].
            let saved = unsafe { core::ptr::read_volatile(fp as *const u64) };
            let ret = unsafe { core::ptr::read_volatile((fp + 8) as *const u64) };
            crate::console::serial_write_fmt_spin(format_args!(
                "[watchpoint]   #{i:02} ret={ret:#x}\n"
            ));
            if saved <= fp {
                break; // frame pointers must strictly increase up the stack
            }
            fp = saved;
            i += 1;
        }
        // Bounded catch: a handful of hits is enough to name the writer's rip
        // (and any legit-writer noise shows as a pattern across them). Disarm
        // after that so a word that turns out to be hot cannot storm the
        // console. `clear_watch` bumps the generation; every CPU drops DR0 on
        // its next timer sync.
        if n >= 6 {
            super::clear_watch();
            crate::console::serial_write_str(
                "[watchpoint] disarming after 6 hits — writer rip(s) captured above\n",
            );
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
