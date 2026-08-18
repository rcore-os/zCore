use crate::context::TrapReason;
use trapframe::TrapFrame;

pub(super) const X86_INT_LOCAL_APIC_BASE: usize = 0xf0;
pub(super) const _X86_INT_APIC_SPURIOUS: usize = X86_INT_LOCAL_APIC_BASE;
pub(super) const X86_INT_APIC_TIMER: usize = X86_INT_LOCAL_APIC_BASE + 0x1;
pub(super) const _X86_INT_APIC_ERROR: usize = X86_INT_LOCAL_APIC_BASE + 0x2;

// ISA IRQ numbers
pub(super) const _X86_ISA_IRQ_PIT: usize = 0;
pub(super) const X86_ISA_IRQ_KEYBOARD: usize = 1;
pub(super) const _X86_ISA_IRQ_PIC2: usize = 2;
pub(super) const X86_ISA_IRQ_COM2: usize = 3;
pub(super) const X86_ISA_IRQ_COM1: usize = 4;
pub(super) const _X86_ISA_IRQ_CMOSRTC: usize = 8;
pub(super) const X86_ISA_IRQ_MOUSE: usize = 12;
pub(super) const _X86_ISA_IRQ_IDE: usize = 14;

fn breakpoint() {
    panic!("\nEXCEPTION: Breakpoint");
}

#[cfg(target_arch = "x86_64")]
fn kernel_trap_name(trap_num: usize) -> &'static str {
    match trap_num {
        0x0 => "#DE divide error",
        0x1 => "#DB debug",
        0x3 => "#BP breakpoint",
        0x4 => "#OF overflow",
        0x5 => "#BR bound range",
        0x6 => "#UD invalid opcode",
        0x7 => "#NM device not available (FPU/SSE?)",
        0x8 => "#DF double fault",
        0x9 => "#MC coprocessor segment overrun",
        0xa => "#TS invalid TSS",
        0xb => "#NP segment not present",
        0xc => "#SS stack fault",
        0xd => "#GP general protection",
        0xe => "#PF page fault",
        0x10 => "#MF x87 FP exception",
        0x11 => "#AC alignment check",
        0x13 => "#XF SIMD FP exception",
        0x100 => "syscall",
        _ => "unknown",
    }
}

pub(super) fn super_timer() {
    crate::timer::timer_tick();
}

/// Resume after skipping a corrupt timer callback: the faulting `call` already
/// pushed its return address; after `iretq` RSP points at that slot, so a
/// single `ret` pops it and continues the `timer_tick` loop.
#[unsafe(naked)]
unsafe extern "C" fn timer_cb_fault_recover() {
    core::arch::naked_asm!("ret");
}

/// `[rsp]` was null (corrupt CALL return). Unused: pop-null recover was
/// removed after it #PF-looped on zero-chains (+0x80). Kept as a labelled
/// stub for crash-site searches.
#[unsafe(naked)]
#[allow(dead_code)]
unsafe extern "C" fn null_slot_fault_recover() {
    core::arch::naked_asm!("ud2",);
}

#[inline]
fn looks_like_kernel_text_ret(ret: u64) -> bool {
    let low32 = ret & 0xffff_ffff;
    let in_text = (0x10_000..0x0100_0000).contains(&low32);
    let high_ok = (ret >> 32) == 0xffff_ff00;
    // Reject RFLAGS-shaped values that can land in the .text low32 window
    // (RF=bit16 ⇒ 0x1xxxx). Real return addresses have kernel high bits.
    let looks_like_rflags = (ret >> 32) == 0 && (ret & 0x2) != 0 && (ret & !0x3f_ffffu64) == 0;
    high_ok && in_text && !looks_like_rflags
}

/// Whether the instruction ending at `ret` is a CALL — i.e. whether `ret` can
/// be a genuine return address at all, rather than a stack qword that merely
/// *ranges* like `.text`.
///
/// The distinction is what separates the two faults this recovery path can be
/// invoked for, and only one of them is repairable:
///
///  * a `call` through a corrupt fn-ptr — the CPU pushed the true return
///    address, `[rsp]` holds it, and resuming there skips exactly one call;
///  * a `ret` that popped garbage (stack overwritten / RSP desynced) — `[rsp]`
///    now holds whatever the *next* stack slot contained. Symbolizing a real
///    capture (ret=0xffffff0000073bf2, three occurrences) against a
///    reproduced ELF put that value in the **middle of a `je`** in
///    `timer_tick`: it was never pushed by any call. "Resuming" at it
///    executes from a byte offset the compiler never emitted — clc/add
///    garbage until the decoder resyncs — which corrupts flags/registers and
///    manufactures the very cascade (three escalating repairs, then a
///    zero-chain crash) the repair exists to prevent.
///
/// A return address is by definition preceded by its CALL, so decode
/// backwards: direct `call rel32` (E8, 5 bytes) or the FF /2 indirect family
/// with optional REX — the encodings rustc emits for `fn` and vtable calls.
/// False negatives (an exotic prefix) leave the repair declined and the fault
/// reported, which is the safe direction; a false positive needs the exact
/// bytes of a CALL immediately before an address the corrupted stack happens
/// to name — vanishingly unlikely, and no worse than today's behaviour.
///
/// `ret` must already have passed [`looks_like_kernel_text_ret`], so the
/// bytes read here are inside mapped, read-only kernel `.text`.
fn preceded_by_call(ret: u64) -> bool {
    let byte = |a: u64| -> u8 {
        // SAFETY: `a` is within [ret - 8, ret), inside mapped kernel .text
        // (caller contract above); .text is never unmapped.
        unsafe { core::ptr::read_volatile(a as *const u8) }
    };
    if ret < 0xffff_ff00_0001_0000 + 8 {
        return false;
    }
    // call rel32: E8 xx xx xx xx
    if byte(ret - 5) == 0xe8 {
        return true;
    }
    // Indirect near call: optional REX (0x40..=0x4F), then FF with ModRM
    // reg-field /2 (0x10..=0x17 register-indirect, 0xD0..=0xD7 register,
    // 0x50..=0x57 [reg+disp8], 0x90..=0x97 [reg+disp32]), possibly with a SIB
    // byte. Instead of fully decoding ModRM/SIB, test each plausible length:
    // an FF at `ret - n` whose ModRM selects /2 and whose operand bytes fill
    // exactly `n` is a CALL ending at `ret`.
    for n in 2..=8u64 {
        let mut p = ret - n;
        let mut rem = n;
        if (0x40..=0x4f).contains(&byte(p)) {
            p += 1;
            rem -= 1;
        }
        if rem < 2 || byte(p) != 0xff {
            continue;
        }
        let modrm = byte(p + 1);
        if (modrm >> 3) & 7 != 2 {
            continue; // not /2 = CALL
        }
        let mode = modrm >> 6;
        let rm = modrm & 7;
        let sib = mode != 3 && rm == 4; // SIB byte present
        let disp: u64 = match mode {
            0 => {
                if rm == 5 {
                    4 // RIP-relative disp32
                } else {
                    0
                }
            }
            1 => 1,
            2 => 4,
            _ => 0, // register-direct
        };
        if 2 + sib as u64 + disp == rem {
            return true;
        }
    }
    false
}

// ── Idle-halt stack seal ────────────────────────────────────────────────────
//
// Four crash captures share one shape: a CPU parked in `Executor::run` →
// `hal_cpu_idle` → `hlt` resumes and a `ret` pops zero from a return slot that
// was overwritten behind its back — `rip=0x0` EXECUTE, `in_timer_callback=
// false`, no current thread, always at the same in-stack depth, on whichever
// executor happened to be parked. A first-generation detector scanned the
// resume window at wake-up for "any `.text` pointer" and stayed silent through
// a full crash: stale call residue above the destroyed slot satisfies it, and
// corruption that happens *after* the wake IRQ (inside the tick work that runs
// on this very stack) postdates the scan entirely.
//
// This is the exact version: **seal** the window above RSP on the way into the
// halt (copy the qwords aside), **verify** it against the copy on the wake IRQ.
// Nothing may legitimately write above a halted CPU's RSP — the wake IRQ's own
// frame goes below — so any difference IS the corruption, reported as
// addr/old/new pairs with the resume point marked. The diff's shape (aligned
// block? object-sized span? ASCII?) is the writer's fingerprint, and "no diff
// during the halt, crash anyway" is equally decisive: it moves the corruption
// window to the wake path itself.
//
// Healthy cost: one 320-byte copy per halt entry and one compare per idle-wake
// IRQ — a few hundred loads/sec per idle CPU.

/// Sealed qwords above the halted RSP. Deliberately larger than the observed
/// zero-span start (the victim slot sits within ~0x30 of the halted RSP).
const IDLE_SEAL_QWORDS: usize = 40; // 320 B

/// One seal per CPU: the snapshot base RSP (0 = disarmed) and the saved window.
/// Single-writer per CPU (its own idle path); the compare runs on the same CPU
/// from the wake IRQ, so `Relaxed` word stores under an acquire/release flag
/// are enough.
struct IdleSeal {
    rsp: core::sync::atomic::AtomicU64,
    words: [core::sync::atomic::AtomicU64; IDLE_SEAL_QWORDS],
}

static IDLE_SEALS: [IdleSeal; 64] = [const {
    IdleSeal {
        rsp: core::sync::atomic::AtomicU64::new(0),
        words: [const { core::sync::atomic::AtomicU64::new(0) }; IDLE_SEAL_QWORDS],
    }
}; 64];

/// Copy the window from `anchor` upward into this CPU's seal.
///
/// Called by `hal_cpu_idle` immediately before its `sti; hlt`, with `anchor`
/// the address of a local variable in `hal_cpu_idle`'s own frame. That choice
/// of base is load-bearing, learned from a live false positive: the first
/// version sealed from this function's RSP, which is one push below the slot
/// where `hal_cpu_idle`'s outgoing `call`s store their return addresses. A
/// second wake IPI (vector 0xf3) that arrived while `call idle_stack_unseal`
/// was re-using that slot compared it against the stale copy and reported the
/// legitimate 7-byte call-site difference (`sti`+`hlt`+`call` = 7 bytes) as
/// corruption — which latched the smash flag and halted an otherwise healthy
/// machine. SysV locals live at or above the frame's RSP while `call` pushes
/// go below it, so an anchor local is on the stable side of that line by ABI,
/// not by compiler mood.
///
/// Everything in `[anchor, anchor + 320)` is genuinely constant while armed:
/// `hal_cpu_idle`'s locals and saved registers (written in its prologue,
/// consumed after disarm), its return slot, and its callers' frames. The only
/// code that runs between seal and disarm is `sti; hlt`, the wake IRQs (whose
/// frames go below the halted RSP), and the `call idle_stack_unseal` push —
/// all below `anchor`.
///
/// Reading 320 bytes upward is in-bounds by construction on the executor idle
/// path: every capture puts the halted RSP ~0x510 below the coroutine stack's
/// top, and the frames of `run_executor`/`Executor::run` above are live
/// mapped stack.
#[inline(never)]
pub(super) fn idle_stack_seal(anchor: usize) {
    use core::sync::atomic::Ordering;
    let cpu = super::cpu::cpu_id() as usize;
    if cpu >= 64 {
        return;
    }
    let base = (anchor as u64) & !7;
    let seal = &IDLE_SEALS[cpu];
    for (i, w) in seal.words.iter().enumerate() {
        let a = base + (i as u64) * 8;
        // SAFETY: 8-aligned, within the live region of this CPU's current
        // stack (see the doc comment).
        w.store(
            unsafe { core::ptr::read_volatile(a as *const u64) },
            Ordering::Relaxed,
        );
    }
    seal.rsp.store(base, Ordering::Release);
}

/// Disarm this CPU's seal — the halt is over and the stack is live again.
pub(super) fn idle_stack_unseal() {
    use core::sync::atomic::Ordering;
    let cpu = super::cpu::cpu_id() as usize;
    if cpu < 64 {
        IDLE_SEALS[cpu].rsp.store(0, Ordering::Release);
    }
}

/// Verify the sealed window on a wake IRQ. Any qword at or above the halted
/// RSP (`tf.rsp`) that differs from its sealed copy was written while the CPU
/// slept — report every such slot while the evidence is fresh.
fn check_idle_seal(tf: &TrapFrame) {
    use core::sync::atomic::{AtomicBool, Ordering};
    let cpu = super::cpu::cpu_id() as usize;
    if cpu >= 64 {
        return;
    }
    let seal = &IDLE_SEALS[cpu];
    let base = seal.rsp.load(Ordering::Acquire);
    if base == 0 {
        return;
    }
    let resume = tf.rsp as u64;
    let mut diffs = 0usize;
    for (i, w) in seal.words.iter().enumerate() {
        let a = base + (i as u64) * 8;
        if a < resume {
            continue; // below the halted RSP: the wake IRQ frame lives here
        }
        // SAFETY: sealed addresses were readable at seal time and are on this
        // CPU's own parked stack.
        let now = unsafe { core::ptr::read_volatile(a as *const u64) };
        let then = w.load(Ordering::Relaxed);
        if now == then {
            continue;
        }
        if diffs == 0 {
            ::executor::note_heap_smash_suspected();
            static REPORTED: AtomicBool = AtomicBool::new(false);
            if REPORTED.swap(true, Ordering::SeqCst) {
                return; // one full report is enough; stay quiet afterwards
            }
            let attr = ::executor::attribute_fault_stack_ptrs(tf.rsp, 0);
            let exec = attr.rsp.map(|h| h.executor_id).unwrap_or(0);
            crate::console::serial_write_fmt_spin(format_args!(
                "\n[idle-smash] CPU{} exec={} stack overwritten WHILE HALTED: \
                 sealed_base={:#x} resume_rsp={:#x} resume_rip={:#x} \
                 wake_vector={:#x} — diffs (addr: sealed -> now):\n",
                cpu, exec, base, resume, tf.rip, tf.trap_num,
            ));
        }
        diffs += 1;
        crate::console::serial_write_fmt_spin(format_args!(
            "[idle-smash]   @{:#x}: {:#018x} -> {:#018x}{}\n",
            a,
            then,
            now,
            if a == resume { "  <-- resume rsp" } else { "" },
        ));
    }
    if diffs > 0 {
        crate::console::serial_write_fmt_spin(format_args!(
            "[idle-smash] {} qword(s) changed during one halt on CPU{}\n",
            diffs, cpu,
        ));
    }
}

/// Usable executor stack with only a shallow nest from `stack_top` (IRQ-sized),
/// not a deep smash into the low-water zone.
fn fault_stack_shallow_usable(rsp: usize) -> bool {
    use ::executor::{StackPtrRegion, STACK_SIZE};
    let attr = ::executor::attribute_fault_stack_ptrs(rsp, 0);
    let Some(h) = attr.rsp else {
        // Outside every executor stack but still a plausible kernel SP —
        // allow idle/IRQ recover (boot / runtime context).
        return true;
    };
    if h.region != StackPtrRegion::Usable {
        return false;
    }
    let stack_top = h.stack_base + STACK_SIZE;
    let used = stack_top.saturating_sub(rsp);
    // ~1.2 KiB below top is the observed null-EXECUTE case; allow up to 16 KiB.
    (8..16 * 1024).contains(&used)
}

/// One-shot: trap vector + error + first 4 stack qwords (CALL vs RET-of-null).
fn dump_null_execute_stack_once(tf: &TrapFrame, sp: u64, slot0: u64) {
    use core::sync::atomic::{AtomicBool, Ordering};
    static DUMPED: AtomicBool = AtomicBool::new(false);
    if DUMPED.swap(true, Ordering::SeqCst) {
        return;
    }
    let mut q = [slot0, 0u64, 0u64, 0u64];
    for i in 1..4 {
        let a = sp + (i * 8) as u64;
        if a >= 0xffff_ff00_0000_0000 && a < 0xffff_ff01_0000_0000 && (a & 7) == 0 {
            // SAFETY: a is 8-aligned in the kernel stack VA window.
            q[i] = unsafe { core::ptr::read_volatile(a as *const u64) };
        }
    }
    crate::console::serial_write_fmt_spin(format_args!(
        "\n[null-exec] trap_num={:#x} error_code={:#x} fault_rsp={:#x} \
         [0]={:#x} [1]={:#x} [2]={:#x} [3]={:#x} \
         (CALL→null keeps return at [0]; RET-of-0 already popped → [0] is next)\n",
        tf.trap_num, tf.error_code, sp, q[0], q[1], q[2], q[3],
    ));
    // The writer's fingerprint is the SHAPE of the zero run, not just the slot
    // that faulted: where it starts and ends, how long it is, and how it is
    // aligned. A page-aligned 0x1000 run is a zeroed frame (a device DMA / a
    // freshly-scrubbed GPU buffer written over a stack); a ~0x400 run is a
    // specific sized buffer; a run that starts mid-qword is a `memset` with a
    // computed length. Scan the contiguous zeros around the fault slot (bounded,
    // 8-aligned reads in the stack window only) and report the extent so a photo
    // of ONE crash is enough to classify the writer without another boot.
    const SW_LO: u64 = 0xffff_ff00_0000_0000;
    const SW_HI: u64 = 0xffff_ff01_0000_0000;
    let readable = |a: u64| a >= SW_LO && a < SW_HI && (a & 7) == 0;
    let read8 = |a: u64| -> Option<u64> {
        // SAFETY: caller-guaranteed 8-aligned address inside the mapped kernel
        // coroutine-stack window; a stray read there cannot fault (the guard
        // pages are outside [SW_LO, SW_HI) growth, and this is diagnostic-only).
        readable(a).then(|| unsafe { core::ptr::read_volatile(a as *const u64) })
    };
    if read8(sp) == Some(0) {
        // Walk down (toward stack_base) and up (toward stack_top) over zeros.
        let mut lo = sp;
        let mut steps = 0u64;
        while steps < 1024 {
            let prev = lo.wrapping_sub(8);
            if read8(prev) == Some(0) {
                lo = prev;
                steps += 1;
            } else {
                break;
            }
        }
        let mut hi = sp;
        steps = 0;
        while steps < 1024 {
            let next = hi.wrapping_add(8);
            if read8(next) == Some(0) {
                hi = next;
                steps += 1;
            } else {
                break;
            }
        }
        let run_len = hi + 8 - lo;
        crate::console::serial_write_fmt_spin(format_args!(
            "[null-exec] zero-run [{:#x}..{:#x}] len={:#x} ({} B) \
             start_align={:#x} (4K-aligned start={}); the bracketing non-zero \
             words below name the buffer edges\n",
            lo,
            hi + 8,
            run_len,
            run_len,
            lo & 0xfff,
            (lo & 0xfff) == 0,
        ));
        // The two words that BRACKET the run (first non-zero on each side) are
        // the survivors the writer did NOT touch — their values often identify
        // the neighbouring live frame (a return address, an Arc, poison).
        if let Some(below) = read8(lo.wrapping_sub(8)) {
            crate::console::serial_write_fmt_spin(format_args!(
                "[null-exec]   below @{:#x} = {:#018x}\n",
                lo - 8,
                below
            ));
        }
        if let Some(above) = read8(hi + 8) {
            crate::console::serial_write_fmt_spin(format_args!(
                "[null-exec]   above @{:#x} = {:#018x}\n",
                hi + 8,
                above
            ));
        }
        // Auto-arm a hardware write-watch DEEP in the zero run (its low, most
        // stack-baseward word — least likely to be rewritten by normal stack use
        // before the writer strikes again). The machine now survives this fault
        // (contained; the single-core TLB wedge is fixed), the desktop respawns
        // and re-triggers the same deterministic corruption, and the write-watch
        // then traps at the writer's EXACT rip — the datum this whole hunt has
        // been missing. Non-invasive: a #DB is delivered after the store retires,
        // so the corruption still lands and the handler only reports; it
        // self-disarms after a few hits (see watchpoint::handle_debug) so it can
        // never storm even if the chosen word turns out to be legitimately
        // written. Costs nothing until it fires.
        if crate::watchpoint::watch_write(lo as usize, 8) {
            crate::console::serial_write_fmt_spin(format_args!(
                "[null-exec] ARMED write-watch on 8B at {:#x} (deep in the zero \
                 run); the writer's NEXT strike traps with its rip — \
                 symbolize [watchpoint] rip with llvm-addr2line -e zcore\n",
                lo,
            ));
        }
    }
    report_dma_uaf_if_recycled(sp);
}

/// Smoking-gun test for the device/mapping use-after-free: was the physical
/// frame backing the faulting stack address a DMA buffer freed recently?
///
/// The physmap stack guard catches a CPU physmap write that aliases a stack,
/// but not a DEVICE DMA or a userspace `VmObject::new_physical` mapping that
/// writes a physical block after `drivers_dma_dealloc` returned it to the pool
/// and `frame_alloc` re-handed it to this coroutine stack. `dma_free_note`
/// records every freed DMA block; if the corrupted stack frame is one of them,
/// this names the writer class the null-execute fault could not otherwise
/// explain. Best-effort and allocation-free — safe on the fault path.
fn report_dma_uaf_if_recycled(sp: u64) {
    use crate::vm::{GenericPageTable, PageTable};
    let va = sp as usize;
    if !(0xffff_ff00_0000_0000..0xffff_ff01_0000_0000).contains(&va) {
        return;
    }
    let pt = PageTable::from_current();
    let paddr = match pt.query(va) {
        Ok((pa, _, _)) => pa,
        Err(_) => {
            core::mem::forget(pt);
            return;
        }
    };
    core::mem::forget(pt);
    if let Some(frees_ago) = crate::stack_guard::paddr_recently_freed_dma(paddr) {
        crate::console::serial_write_fmt_spin(format_args!(
            "[dma-uaf] the corrupted stack frame pa={:#x} was a DMA buffer freed \
             {} DMA-free(s) ago — a device descriptor or a userspace \
             VmObject::new_physical mapping wrote it AFTER free: THIS is the \
             zero-writer (a freed GEM/ring recycled into a live coroutine stack)\n",
            paddr, frees_ago,
        ));
    }
}

/// `[rsp]==0` null-EXECUTE on a shallow/usable stack: scan upward for a kernel
/// `.text` return and RET there. Never "pop-null recover" — a sea of zeros
/// (fresh BSS stack / RSP past live frames) turns that into an 8× #PF storm
/// (`fault_rsp` advancing 0x80) before halt.
fn try_recover_null_return_slot(tf: &mut TrapFrame, fault_vaddr: usize, sp: u64) -> bool {
    if !fault_stack_shallow_usable(tf.rsp) {
        return false;
    }

    // If the next slot is also 0, this is a zero-chain (not a single bad CALL
    // return). Popping one null only RETs into the next — abort that path.
    // SAFETY: sp+8 is still in the kernel stack window when sp is.
    let next = unsafe { core::ptr::read_volatile((sp + 8) as *const u64) };
    if next == 0 {
        use core::sync::atomic::{AtomicBool, Ordering};
        static LOGGED: AtomicBool = AtomicBool::new(false);
        if !LOGGED.swap(true, Ordering::SeqCst) {
            crate::console::serial_write_fmt_spin(format_args!(
                "\n[null-exec] zero-chain at fault_rsp={:#x} ([0]=[1]=0) — refusing \
                 pop-null recover (would #PF-loop); vaddr={:#x}\n",
                sp, fault_vaddr,
            ));
            // [diag] Name the timer callback in flight, if any: the reproduced
            // crash has in_timer_callback=true, and this pair — published by
            // timer_tick around each dispatch — turns "somewhere in the tick"
            // into one closure. Symbolize the vtable against the kernel ELF.
            let (cb_data, cb_vtable) = crate::kstats::current_timer_cb();
            crate::console::serial_write_fmt_spin(format_args!(
                "[null-exec] timer callback in flight: data={:#x} vtable={:#x} \
                 (0,0 = fault is outside any timer callback dispatch)\n",
                cb_data, cb_vtable,
            ));
            // [diag] Full window dump, EVERY qword — the pointer-only scan
            // that follows hides the zero-span's exact boundaries, and those
            // boundaries are the writer's fingerprint (buffer-sized? page-
            // aligned? where does it start relative to the ret slot?).
            let lo = sp.saturating_sub(0x40) & !0x7;
            let hi = sp + 0x1c0;
            crate::console::serial_write_fmt_spin(format_args!(
                "[null-exec] window [{:#x}..{:#x}]:\n",
                lo, hi,
            ));
            let mut a = lo;
            while a < hi {
                if a >= 0xffff_ff00_0000_0000 && a < 0xffff_ff01_0000_0000 {
                    // SAFETY: 8-aligned address on this executor's mapped stack
                    // window (same bound as the scan below).
                    let w = unsafe { core::ptr::read_volatile(a as *const u64) };
                    crate::console::serial_write_fmt_spin(format_args!(
                        "[null-exec]   @{:#x} = {:#018x}{}\n",
                        a,
                        w,
                        if a == sp { "  <-- fault rsp" } else { "" },
                    ));
                }
                a += 8;
            }
        }
        return false;
    }

    // Scan toward stack_top for a plausible CALL return (0xffff_ff00_00xxxxxx).
    const SCAN_QWORDS: u64 = 32;
    for i in 1..=SCAN_QWORDS {
        let addr = sp + i * 8;
        if (addr & 7) != 0 || addr >= 0xffff_ff01_0000_0000 {
            break;
        }
        // SAFETY: addr stays in the same kernel stack window as `sp`.
        let cand = unsafe { core::ptr::read_volatile(addr as *const u64) };
        if looks_like_kernel_text_ret(cand) && preceded_by_call(cand) {
            // `trap_return` ignores software tf.rsp — plant the return at [sp]
            // so `timer_cb_fault_recover`'s `ret` resumes there.
            // SAFETY: sp is the faulting RSP slot we already validated.
            unsafe { core::ptr::write_volatile(sp as *mut u64, cand) };
            use core::sync::atomic::{AtomicUsize, Ordering};
            static RECOVERS: AtomicUsize = AtomicUsize::new(0);
            let n = RECOVERS.fetch_add(1, Ordering::Relaxed);
            if n < 32 {
                crate::console::serial_write_fmt_spin(format_args!(
                    "\n[null-exec] recovered null-[rsp] EXECUTE vaddr={:#x}: \
                     planted ret={:#x} from +{} (scan); rip -> recover\n",
                    fault_vaddr,
                    cand,
                    i * 8,
                ));
            }
            if crate::timer::in_timer_callback() {
                crate::timer::note_timer_callback_skipped();
            }
            tf.rip = timer_cb_fault_recover as *const () as usize;
            return true;
        }
    }

    false
}

/// Try to contain a null-range EXECUTE #PF from a bad `call` (corrupt fn-ptr /
/// vtable). Returns `true` if the trap frame was rewritten to skip the call
/// (caller must `return` from `trap_handler` without panicking).
///
/// Safe when `[tf.rsp]` still holds a full kernel `.text` return address
/// (the qword the CALL pushed). Also recovers a **null** return slot on a
/// shallow/usable stack by scanning upward or popping the null (idle IRQ).
/// Truncated residues like a real smash return false.
///
/// `tf.rsp` must be the **faulting RSP value** (see trap.S `__from_kernel`);
/// historically it was `&rflags`, which made RFLAGS|RF (e.g. `0x13446`) look
/// like truncated `.text` and false-triggered soft-smash.
fn try_skip_null_execute_call(tf: &mut TrapFrame, fault_vaddr: usize) -> bool {
    // For same-CPL kernel #PF on `call`, CPU-saved RSP points at the return
    // address the CALL pushed before loading the bad RIP.
    let sp = tf.rsp as u64;
    let plausible_sp = |a: u64| a >= 0xffff_ff00_0000_0000 && a < 0xffff_ff01_0000_0000;
    if !plausible_sp(sp) || (sp & 7) != 0 {
        return false;
    }
    // SAFETY: sp is the faulting RSP from the hardware iret frame (trap.S),
    // 8-aligned and in the kernel heap/stack range.
    let ret = unsafe { core::ptr::read_volatile(sp as *const u64) };

    // Null return slot: not truncated smash residue — try shallow-stack recover
    // before sticky-flagging heap smash (that would kill all idle dyn dispatch).
    if ret == 0 {
        dump_null_execute_stack_once(tf, sp, ret);
        if try_recover_null_return_slot(tf, fault_vaddr, sp) {
            return true;
        }
        // Unrecoverable null slot — treat like soft-smash for skip_dyn.
        ::executor::note_heap_smash_suspected();
        use core::sync::atomic::{AtomicBool, Ordering};
        static LOGGED_NULL: AtomicBool = AtomicBool::new(false);
        if !LOGGED_NULL.swap(true, Ordering::SeqCst) {
            let (hard, soft) = ::executor::hard_guard_executor_counts();
            crate::console::serial_write_fmt_spin(format_args!(
                "\n[soft-smash] null return slot [fault_rsp]={:#x} — \
                 hooks_registered={} hard_guard_executors={} soft_guard_executors={}\n",
                sp,
                ::executor::stack_guard_hooks_registered(),
                hard,
                soft,
            ));
            let attr = ::executor::attribute_fault_stack_ptrs(tf.rsp, tf.rbp);
            if let Some(h) = attr.rsp {
                crate::console::serial_write_fmt_spin(format_args!(
                    "[soft-smash] rsp={:#x} -> CPU{} exec={} region={}\n",
                    tf.rsp,
                    h.cpu,
                    h.executor_id,
                    h.region.as_str(),
                ));
            }
        }
        return false;
    }

    // Accept only a return into kernel .text (same bound the #GP-repair path
    // uses for low32). A non-kernel / truncated ret means the stack is too
    // far gone to skip safely — fall through to the panic diagnostics.
    let low32 = ret & 0xffff_ffff;
    let in_text = (0x10_000..0x0100_0000).contains(&low32);
    let high_ok = (ret >> 32) == 0xffff_ff00;
    let looks_like_rflags = (ret >> 32) == 0 && (ret & 0x2) != 0 && (ret & !0x3f_ffffu64) == 0;
    if !high_ok || !in_text || looks_like_rflags {
        let truncated_text = (ret >> 32) == 0 && in_text && !looks_like_rflags;
        if truncated_text || ret < 0x1000 {
            ::executor::note_heap_smash_suspected();
            use core::sync::atomic::{AtomicBool, Ordering};
            static LOGGED: AtomicBool = AtomicBool::new(false);
            if !LOGGED.swap(true, Ordering::SeqCst) {
                let (hard, soft) = ::executor::hard_guard_executor_counts();
                crate::console::serial_write_fmt_spin(format_args!(
                    "\n[soft-smash] [fault_rsp]={:#x} value={:#x} — hooks_registered={} \
                     hard_guard_executors={} soft_guard_executors={}\n",
                    sp,
                    ret,
                    ::executor::stack_guard_hooks_registered(),
                    hard,
                    soft,
                ));
                let attr = ::executor::attribute_fault_stack_ptrs(tf.rsp, tf.rbp);
                let report =
                    |label: &str, addr: usize, hit: Option<::executor::StackAttrHit>| match hit {
                        Some(h) => crate::console::serial_write_fmt_spin(format_args!(
                            "[soft-smash] {}={:#x} -> CPU{} exec={} task={} \
                             stack_base={:#x} region={}\n",
                            label,
                            addr,
                            h.cpu,
                            h.executor_id,
                            h.task_id,
                            h.stack_base,
                            h.region.as_str(),
                        )),
                        None => crate::console::serial_write_fmt_spin(format_args!(
                            "[soft-smash] {}={:#x} -> OUTSIDE all executor stacks \
                             (walked {} CPUs, skipped {}, {} executors)\n",
                            label, addr, attr.cpus_walked, attr.cpus_skipped, attr.executors_seen,
                        )),
                    };
                report("rsp", tf.rsp, attr.rsp);
                report("rbp", tf.rbp, attr.rbp);
            }
            // Only halt early on *confirmed* truncated residue (not RFLAGS).
            // Null fn-ptr with a valid return still falls through to skip below
            // when high_ok; when residue is real, sticky + optional timer halt.
            if truncated_text && crate::timer::in_timer_callback() {
                crate::timer::note_timer_callback_skipped();
                crate::console::serial_write_str(
                    "\n[soft-smash] truncated return while in_timer_callback — \
                     sticky set; serial halt\n",
                );
                loop {
                    core::hint::spin_loop();
                }
            }
        }
        return false;
    }

    // Range-plausible is not enough — the value must be a return address some
    // CALL actually produced. The three-repair cascade symbolized in
    // `preceded_by_call`'s doc came through here: a RET popped garbage, the
    // "return" this read was the *next* stack slot (mid-instruction residue),
    // and resuming at it executed byte salad from inside a `je`. When the
    // check fails, the fault is a corrupted-stack RET, and there is no
    // instruction stream to resume — report it as what it is and let the
    // fall-through diagnostics + halt handle it.
    if !preceded_by_call(ret) {
        use core::sync::atomic::{AtomicBool, Ordering};
        ::executor::note_heap_smash_suspected();
        static LOGGED_RESIDUE: AtomicBool = AtomicBool::new(false);
        if !LOGGED_RESIDUE.swap(true, Ordering::SeqCst) {
            crate::console::serial_write_fmt_spin(format_args!(
                "\n[null-exec] [fault_rsp]={:#x} ranges like .text but is NOT \
                 preceded by a CALL — a RET went through a corrupted stack slot \
                 (RSP desync / stack overwrite), not a bad fn-ptr call. \
                 Refusing to resume at residue (vaddr={:#x} rip={:#x}).\n",
                ret, fault_vaddr, tf.rip,
            ));
        }
        return false;
    }

    use core::sync::atomic::{AtomicUsize, Ordering};
    static REPORTS: AtomicUsize = AtomicUsize::new(0);
    let n = REPORTS.fetch_add(1, Ordering::Relaxed);
    if n < 64 {
        crate::console::serial_write_fmt_spin(format_args!(
            "\n[KERNEL BUG] null-range EXECUTE #PF (vaddr={:#x} rip={:#x}); \
             skipping bad call -> ret={:#x} (in_timer_callback={}). \
             Interrupted userspace thread name is coincidental — NOT a \
             userspace-caused halt.\n",
            fault_vaddr,
            tf.rip,
            ret,
            crate::timer::in_timer_callback(),
        ));
    }
    if crate::timer::in_timer_callback() {
        crate::timer::note_timer_callback_skipped();
    }
    tf.rip = timer_cb_fault_recover as *const () as usize;
    true
}

#[no_mangle]
pub extern "C" fn trap_handler(tf: &mut TrapFrame) {
    trace!(
        "Interrupt: {:#x} @ CPU{}",
        tf.trap_num,
        super::cpu::cpu_id()
    );

    // [diag] NMI (vector 2) is used only as a "where are you stuck?" probe: it is
    // delivered even to a core spinning with interrupts disabled. Record the
    // interrupted RIP and return — do NOT fall through to the GernelFault panic.
    if tf.trap_num == 2 {
        crate::kstats::note_nmi_rip(tf.rip as u64);
        return;
    }

    // [diag] A data watchpoint (DR0-DR3) reports as #DB. It is a *trap*: the
    // store has already retired, so once the hit is reported the interrupted
    // code resumes. Checked before the generic breakpoint panic below, which
    // would otherwise kill the machine on the very fault we armed to observe.
    if tf.trap_num == 0x1
        && crate::watchpoint::handle_debug_trap(tf.rip as u64, tf.rsp as u64, tf.rbp as u64)
    {
        return;
    }

    match TrapReason::from(tf.trap_num, tf.error_code) {
        TrapReason::HardwareBreakpoint | TrapReason::SoftwareBreakpoint => breakpoint(),
        TrapReason::PageFault(vaddr, flags) => {
            // [diag] Stash the faulting instruction pointer so the kernel-side
            // page-fault handler can name the exact code that faulted (the
            // handler only receives vaddr+flags, not the trap frame). Read
            // back in ZcoreKernelHandler::handle_page_fault before it panics.
            // Cheap: one relaxed store per fault, overwritten on the next.
            // Capture rbp/rsp too so the handler can walk the faulting call
            // chain (e.g. name the caller of a wild `memset`, tf.rip resolving
            // into compiler_builtins set_bytes).
            crate::kstats::note_fault_regs(tf.rip as u64, tf.rbp as u64, tf.rsp as u64);
            // Containment: null-range EXECUTE #PF is a corrupt fn-ptr / vtable
            // call. Skip when the pushed return address is still a valid
            // kernel `.text` pointer (timer callbacks AND other IRQ/kernel
            // indirect calls). Truncated [rsp]=0x13446 means smash — do not
            // skip. Real user VMAR faults are untouched (vaddr >= 0x1000).
            if vaddr < 0x1000
                && flags.contains(crate::MMUFlags::EXECUTE)
                && try_skip_null_execute_call(tf, vaddr)
            {
                return;
            }
            crate::KHANDLER.handle_page_fault(vaddr, flags)
        }
        TrapReason::Interrupt(vector) => {
            // [diag] Attribute the scheduler tick to the context it interrupted:
            // ring 3 (CS low 2 bits == 3) means a user thread was running; ring 0
            // means kernel (idle hlt / syscall / kernel spin). Localises a pegged
            // core's busy time to user vs kernel for /proc/perf.
            if vector == X86_INT_APIC_TIMER {
                crate::kstats::note_tick_context(tf.cs & 0b11 == 0b11, tf.rip as u64);
            }
            // Woke a halted idle executor: verify its resume path is still a
            // callable return chain BEFORE running any tick work on it (and
            // long before the doomed `ret` would execute). See the function's
            // doc for the crash shape this catches at the earliest possible
            // observation point.
            if tf.cs & 0b11 == 0b00 && crate::kstats::current_cpu_in_idle() {
                check_idle_seal(tf);
            }
            // Coroutine-stack tripwires on *every* IRQ, not only the APIC
            // timer: PS/2 / UART / xHCI nest on the same executor stack and
            // were the path to `rip=0` / `[rsp0]=0x13446` with
            // `in_timer_callback=false` after a silent overflow between ticks.
            // Soft canary is a no-op under hard_guard; proximity still fires.
            executor::check_current_executor_canary();
            if tf.cs & 0b11 == 0b00 {
                executor::check_current_executor_stack_proximity(tf.rsp);
            }
            crate::interrupt::handle_irq(vector);
            // Second seal check, AFTER the IRQ work. The entry check above
            // covers "overwritten while halted"; this one covers "overwritten
            // by this very interrupt's own work" — the tick machinery (net
            // poll, input poll, event listeners, timer callbacks) runs right
            // here on the parked executor's stack, below the sealed window. A
            // diff that appears only now was written by something this tick
            // executed, which narrows the writer to one interrupt's worth of
            // code instead of a whole halt.
            if tf.cs & 0b11 == 0b00 && crate::kstats::current_cpu_in_idle() {
                check_idle_seal(tf);
            }
            // Timer preemption is handled in the thread trap path (e.g.
            // `loader/src/linux.rs` calls `yield_now` on TIMER). Never call
            // `executor::handle_timeout()` here: it context-switches from IRQ
            // context and abandons the trap frame → triple fault (QEMU/VBox).
        }
        TrapReason::GernelFault(vec) => {
            // [mitigation] Recover the executor-stack return-address corruption
            // reproduced by `timeout -s TERM 1 sleep 5`. A `ret`/jump lands on a
            // saved kernel code pointer (0xffffff00_00xxxxxx) whose top byte was
            // overwritten (ff -> 01/0a/...): the target is non-canonical, raising
            // #GP with tf.rip = the mangled address. This handler runs on the
            // dedicated #GP IST stack (idt.rs vector 13), so it executes despite
            // the corrupt stack. Un-mangle the top byte and resume: the `ret`
            // then effectively lands where it was meant to, with the rest of the
            // (otherwise intact) stack. Signature is tight — the 0xffff00 middle
            // pattern of a real kernel pointer AND a low32 inside .text — so it
            // never fires on a genuine #GP or user address.
            if vec == 13 {
                let rip = tf.rip;
                let mid_is_kernel = ((rip >> 32) & 0x00ff_ffff) == 0x00ff_ff00;
                let top_mangled = (rip >> 56) != 0xff;
                let low32 = rip & 0xffff_ffff;
                let in_text = (0x10_000..0x0100_0000).contains(&low32);
                if mid_is_kernel && top_mangled && in_text {
                    use core::sync::atomic::{AtomicUsize, Ordering};
                    static REPAIRS: AtomicUsize = AtomicUsize::new(0);
                    let fixed = (rip & 0x00ff_ffff_ffff_ffff) | 0xff00_0000_0000_0000;
                    let n = REPAIRS.fetch_add(1, Ordering::Relaxed);
                    if n < 64 {
                        crate::console::serial_write_fmt_spin(format_args!(
                            "\n[#GP-repair #{}] mangled kernel RIP {:#x} -> {:#x}; resuming\n",
                            n, rip, fixed,
                        ));
                    }
                    tf.rip = fixed;
                    return;
                }
            }
            // x86 CPU exception — translate the vector to a readable name so
            // the panic message is immediately actionable without a debugger.
            let name = match vec {
                0 => "Divide Error (#DE)",
                1 => "Debug (#DB)",
                2 => "NMI",
                3 => "Breakpoint (#BP)",
                4 => "Overflow (#OF)",
                5 => "Bound Range Exceeded (#BR)",
                6 => "Invalid Opcode (#UD)",
                7 => "Device Not Available / No Math Coprocessor (#NM)",
                8 => "Double Fault (#DF)",
                9 => "Coprocessor Segment Overrun",
                10 => "Invalid TSS (#TS)",
                11 => "Segment Not Present (#NP)",
                12 => "Stack Segment Fault (#SS)",
                13 => "General Protection Fault (#GP)",
                14 => "Page Fault (#PF via GernelFault — should not happen)",
                16 => "x87 FPU Floating-Point Error (#MF)",
                17 => "Alignment Check (#AC)",
                18 => "Machine Check (#MC)",
                19 => "SIMD Floating-Point Exception (#XF)",
                _ => "Unknown CPU exception",
            };
            // Stash the faulting frame BEFORE panicking. A #DF (and #GP) runs
            // on its own IST stack, so by the time `oops` looks at the current
            // SP it can no longer tell which coroutine stack actually faulted.
            // This is what lets containment abandon that executor from the IST
            // stack instead of halting the machine.
            crate::kstats::note_fault_regs(tf.rip as u64, tf.rbp as u64, tf.rsp as u64);
            panic!(
                "\nCPU EXCEPTION on CPU{}: {} (vec={:#x})\n\
                 error_code={:#x}\n{:#x?}",
                super::cpu::cpu_id(),
                name,
                vec,
                tf.error_code,
                tf
            );
        }
        TrapReason::UndefinedInstruction => panic!(
            "\nCPU EXCEPTION on CPU{}: Invalid Opcode (#UD) at RIP={:#x}\n{:#x?}",
            super::cpu::cpu_id(),
            tf.rip,
            tf
        ),
        TrapReason::UnalignedAccess => panic!(
            "\nCPU EXCEPTION on CPU{}: Alignment Check (#AC) at RIP={:#x}\n{:#x?}",
            super::cpu::cpu_id(),
            tf.rip,
            tf
        ),
        TrapReason::Syscall => panic!(
            "\nCPU EXCEPTION on CPU{}: Syscall trap in kernel context at RIP={:#x}\n{:#x?}",
            super::cpu::cpu_id(),
            tf.rip,
            tf
        ),
    }
}
