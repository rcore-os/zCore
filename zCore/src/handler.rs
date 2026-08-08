use core::sync::atomic::{AtomicBool, Ordering};

use kernel_hal::{KernelHandler, MMUFlags};
use zircon_object::object::KernelObject;
use zircon_object::task::Thread;

use super::memory;

pub struct ZcoreKernelHandler;

/// Set while we are already diagnosing a null-range #PF. A second fault during
/// `format_args!` / `panic!` (heap/vtable already smashed) must NOT recurse into
/// another formatted dump — that was the infinite `[KERNEL PAGE FAULT]` cascade
/// after the first null fn-ptr, especially under DRM redraw storms (QEMU resize).
static FAULT_DIAG_ACTIVE: AtomicBool = AtomicBool::new(false);

impl KernelHandler for ZcoreKernelHandler {
    fn frame_alloc(&self) -> Option<usize> {
        memory::frame_alloc(1, 0)
    }

    fn frame_alloc_contiguous(&self, frame_count: usize, align_log2: usize) -> Option<usize> {
        memory::frame_alloc(frame_count, align_log2)
    }

    fn frame_dealloc(&self, paddr: usize) {
        memory::frame_dealloc(paddr)
    }

    fn handle_page_fault(&self, fault_vaddr: usize, access_flags: MMUFlags) {
        // Unmapped coroutine stack guard: overflow hit the hard guard instead of
        // smashing heap into a later null fn-ptr (`rip=0x3`). Panic with a clear
        // label — do not attempt VMAR resolution.
        #[cfg(not(feature = "libos"))]
        if kernel_hal::stack_guard::is_guard_fault(fault_vaddr) {
            let rip = kernel_hal::kstats::last_fault_rip();
            panic!(
                "\n[stack-guard] COROUTINE STACK OVERFLOW: fault_vaddr={:#x} \
                 access={:?} fault_rip={:#x} — growth hit an unmapped \
                 bottom/top guard around the executor stack (prevented heap smash)\n",
                fault_vaddr, access_flags, rip
            );
        }
        // Guard: very low addresses (null-pointer dereference with a field offset)
        // are never valid user or kernel mappings — they indicate a use-after-free
        // or corrupted pointer somewhere. Attempting to resolve them through the
        // thread's vmar will itself fault (the vmar/process may be the freed
        // object), causing re-entrant page faults that cascade across all CPUs.
        // Catch them early and report without touching any thread/process state.
        if fault_vaddr < 0x1000 {
            // Re-entrant path: formatting the first fault already corrupted the
            // heap enough that `format_args!`/`Write` vtables are NULL. A second
            // entry must halt with a literal string only — no fmt, no panic!.
            if FAULT_DIAG_ACTIVE.swap(true, Ordering::SeqCst) {
                kernel_hal::console::serial_write_str(
                    "\n[KERNEL PAGE FAULT] re-entrant null-range while diagnosing — halting\n",
                );
                loop {
                    core::hint::spin_loop();
                }
            }
            let in_timer = kernel_hal::timer::in_timer_callback();
            // ONLY the spin serial writer: `console_write_fmt` goes through the
            // graphic console trait object and was observed to #PF again
            // (null vtable) while diagnosing — the
            // "re-entrant null-range while diagnosing" loop.
            kernel_hal::console::serial_write_fmt_spin(format_args!(
                "\n[KERNEL BUG] null-range #PF vaddr={:#x} flags={:?} rip={:#x} \
                 (kernel-context fault — not a userspace SIGSEGV; \
                 in_timer_callback={}; not retriable)\n",
                fault_vaddr,
                access_flags,
                kernel_hal::kstats::last_fault_rip(),
                in_timer,
            ));
            // Name the interrupted thread via serial only (never graphic fmt).
            if let Some(thread) = kernel_hal::thread::get_current_thread() {
                if let Ok(thread) = thread.downcast::<Thread>() {
                    kernel_hal::console::serial_write_fmt_spin(format_args!(
                        "[diag] interrupted thread (coincidental if IRQ/timer): \
                         {:?} \"{}\" in process \"{}\"{}\n",
                        thread.id(),
                        thread.name(),
                        thread.proc().name(),
                        if in_timer {
                            " — KERNEL BUG in timer callback, not this process"
                        } else {
                            ""
                        },
                    ));
                }
            } else {
                kernel_hal::console::serial_write_str(
                    "[diag] no current thread (IRQ / early boot / private kernel)\n",
                );
            }
            // Note: historical `[rsp0]=0x13446` / `0x13486` logs were often
            // RFLAGS|RF misread as a return address (trap.S stored &rflags in
            // tf.rsp). After the fix, rsp0 is the real faulting RSP; truncated
            // .text residue here is genuine smash — do not walk that chain.
            print_fault_backtrace(access_flags);
            // Prefer a literal halt over `panic!` here: `panic!` formats through
            // the global panic handler and can #PF again on a smashed heap,
            // which only produces the re-entrant spin with less context.
            kernel_hal::console::serial_write_str(
                if in_timer {
                    "\n[KERNEL BUG] null-range #PF inside timer path — halting \
                     (not caused by the interrupted userspace process)\n"
                } else {
                    "\n[KERNEL BUG] null-range kernel #PF — halting \
                     (not a userspace-caused halt)\n"
                },
            );
            loop {
                core::hint::spin_loop();
            }
        }

        if let Some(thread) = kernel_hal::thread::get_current_thread() {
            if let Ok(thread) = thread.downcast::<Thread>() {
                let vmar = thread.proc().vmar();
                if let Err(err) = vmar.handle_page_fault(fault_vaddr, access_flags) {
                    // Loud on the *graphic* console (panic prints to serial only,
                    // invisible on a headless-but-monitor'd bring-up box). A kernel
                    // fault here during a syscall is almost always a driver
                    // dereferencing an address the user vmar can't resolve -- e.g.
                    // the vendored NVIDIA RM touching an unmapped/mismapped MMIO or
                    // heap pointer. The fault address + flags name the culprit
                    // directly instead of leaving a silent frozen `cat`.
                    kernel_hal::console::console_write_fmt(format_args!(
                        "\n[KERNEL PAGE FAULT] vaddr={:#x} flags={:?} err={:?} rip={:#x} \
                         (unresolved against the faulting thread's user vmar -- \
                         likely a kernel-side driver bug, not a userspace fault)\n",
                        fault_vaddr,
                        access_flags,
                        err,
                        kernel_hal::kstats::last_fault_rip(),
                    ));
                    // [diag] Same frame-pointer/raw-stack walk as the kernel-private
                    // branch below -- this fault has a current thread, but the RIP
                    // landing outside .text (e.g. a corrupted fn-ptr/vtable jumping
                    // into .rodata) is exactly the case where naming the *caller*
                    // matters most. See the shared helper's doc comment.
                    print_fault_backtrace(access_flags);
                    panic!(
                        "handle kernel page fault error: {:?} vaddr(0x{:x}) flags({:?})",
                        err, fault_vaddr, access_flags
                    );
                }
            }
        } else {
            kernel_hal::console::console_write_fmt(format_args!(
                "\n[KERNEL PAGE FAULT] vaddr={:#x} flags={:?} rip={:#x} \
                 (no current thread -- fault in kernel-private context)\n",
                fault_vaddr,
                access_flags,
                kernel_hal::kstats::last_fault_rip(),
            ));
            print_fault_backtrace(access_flags);
            panic!(
                "page fault from kernel private address 0x{:x}, flags = {:?}, rip = {:#x}",
                fault_vaddr,
                access_flags,
                kernel_hal::kstats::last_fault_rip(),
            );
        }
    }

    fn memory_usage(&self) -> (usize, usize) {
        memory::stats()
    }
}

/// [diag] Walk the frame-pointer chain from the faulting instruction and print
/// the return addresses, then fall back to a raw stack scan for kernel code
/// pointers. The wild write reproduced by `cat /proc/self/exe` faulted inside
/// compiler_builtins `set_bytes` (memset) writing to a corrupted kernel
/// destination; its own frame has no useful name, so the CALLER chain printed
/// here is what names the code that handed memset the bad pointer/length.
/// Same idea applies to an RIP that lands outside `.text` entirely (a
/// corrupted function pointer/vtable): the faulting frame itself is garbage,
/// but its caller's return address on the stack usually still is not. Uses
/// the spin/blocking serial writer so this survives even as the panic that
/// follows re-faults on a corrupted stack. rbp==0 or a scan that leaves the
/// plausible kernel range simply stops the walk.
fn print_fault_backtrace(access_flags: MMUFlags) {
    let rbp0 = kernel_hal::kstats::last_fault_rbp();
    let rsp0 = kernel_hal::kstats::last_fault_rsp();
    // Early diagnosis: after trap.S fix, rsp0 is the faulting RSP value and
    // [rsp0] is the top-of-stack qword (CALL return for null EXECUTE). A
    // truncated .text low32 with high word zero used to be blamed on stack
    // overflow — but before the fix, tf.rsp was *&rflags* and RFLAGS|RF
    // (often 0x13xxx) falsely matched the same pattern. Skip RFLAGS-shaped
    // values; with hard guards, real truncated residue means in-stack smash
    // or heap corruption, not growth past unmapped guards.
    {
        // Align with an explicit u64 mask — `rsp0` is u64; `!(0x7usize)` would
        // be a type error against it.
        let sp0 = rsp0 & !0x7u64;
        let plausible_sp = |a: u64| a >= 0xffff_ff00_0000_0000 && a < 0xffff_ff01_0000_0000;
        if plausible_sp(sp0) {
            let top = unsafe { core::ptr::read_volatile(sp0 as *const u64) };
            let looks_like_rflags =
                (top >> 32) == 0 && (top & 0x2) != 0 && (top & !0x3f_ffffu64) == 0;
            let truncated_text = (top >> 32) == 0
                && (0x10_000u64..0x0100_0000u64).contains(&(top & 0xffff_ffff))
                && !looks_like_rflags;
            if truncated_text {
                // Soft-smash (NOT unmapped [stack-guard] #PF): sticky + mode proof.
                #[cfg(not(feature = "libos"))]
                {
                    executor::note_heap_smash_suspected();
                    let (hard, soft) = executor::hard_guard_executor_counts();
                    if hard > 0 {
                        kernel_hal::console::serial_write_fmt_spin(format_args!(
                            "[diag] truncated return residue (rsp0={:#x} [rsp0]={:#x}); \
                             hard guards not hit — likely in-stack buffer smash or \
                             heap fn-ptr corruption (NOT coroutine stack overflow)\n",
                            sp0, top,
                        ));
                    } else {
                        kernel_hal::console::serial_write_fmt_spin(format_args!(
                            "[diag] likely coroutine stack overflow — return address high \
                             word was zeroed (rsp0={:#x} [rsp0]={:#x}); the growing stack \
                             overwrote its own return-address slot with the low half of a \
                             kernel .text pointer\n",
                            sp0, top,
                        ));
                    }
                    kernel_hal::console::serial_write_fmt_spin(format_args!(
                        "[soft-smash] hooks_registered={} hard_guard_executors={} \
                         soft_guard_executors={}\n",
                        executor::stack_guard_hooks_registered(),
                        hard,
                        soft,
                    ));
                    report_soft_smash_stack_attr(rsp0 as usize, rbp0 as usize);
                    // Only halt early inside timer — hard-guard-only halt used to
                    // fire on the RFLAGS false-positive and mask the real null call.
                    if kernel_hal::timer::in_timer_callback() {
                        kernel_hal::timer::note_timer_callback_skipped();
                        kernel_hal::console::serial_write_str(
                            "\n[soft-smash] truncated return while in_timer_callback — \
                             sticky set; serial halt\n",
                        );
                        loop {
                            core::hint::spin_loop();
                        }
                    }
                }
                #[cfg(feature = "libos")]
                {
                    kernel_hal::console::serial_write_fmt_spin(format_args!(
                        "[diag] truncated return residue (rsp0={:#x} [rsp0]={:#x})\n",
                        sp0, top,
                    ));
                }
            }
        }
    }
    kernel_hal::console::serial_write_fmt_spin(format_args!(
        "[kfault-bt] rbp={:#x} rsp={:#x} walking frames:\n",
        rbp0, rsp0,
    ));
    // Two separate bounds, deliberately different widths:
    //  - `plausible` (rbp walk): kept at the original, narrower 256 MiB. This
    //    loop chases whatever value is STORED at each frame -- if that chain
    //    is corrupted (as observed: captures so far show it going nowhere
    //    useful within 1-2 hops), an untrusted `saved_rbp` could point
    //    anywhere; a tighter bound limits how far a bad chain can wander
    //    before this loop's own gate stops it from dereferencing further.
    //  - `plausible_sp` (below): only ever applied to `rsp0` itself and small
    //    fixed offsets from it (the stack scan advances by 8 bytes at a time,
    //    at most 4 KiB total) -- addresses that stay local to a known-live
    //    pointer regardless of how wide this bound is, so widening it here
    //    doesn't carry the rbp-walk's wander risk. See its own comment below
    //    for why it needed widening at all.
    let plausible = |a: u64| a >= 0xffff_ff00_0000_0000 && a < 0xffff_ff00_1000_0000;
    let plausible_sp = |a: u64| a >= 0xffff_ff00_0000_0000 && a < 0xffff_ff01_0000_0000;
    // Smash signature: [rsp0] truncated to a .text low32 (not RFLAGS-shaped)
    // or a non-kernel value. Walking that as a frame chain #PF's the diag path
    // (re-entrant null-range). Report once and stop.
    let sp0 = rsp0 & !0x7u64;
    if plausible_sp(sp0) && (sp0 & 7) == 0 {
        let top = unsafe { core::ptr::read_volatile(sp0 as *const u64) };
        let looks_like_rflags =
            (top >> 32) == 0 && (top & 0x2) != 0 && (top & !0x3f_ffffu64) == 0;
        let truncated_text = (top >> 32) == 0
            && (0x10_000..0x0100_0000).contains(&(top & 0xffff_ffff))
            && !looks_like_rflags;
        if top < 0x1000 || truncated_text {
            kernel_hal::console::serial_write_fmt_spin(format_args!(
                "[kfault-bt] stack smash residue [rsp0]={:#x}; skipping frame walk \
                 (avoids re-entrant #PF)\n",
                top,
            ));
            return;
        }
    } else {
        kernel_hal::console::serial_write_fmt_spin(format_args!(
            "[kfault-bt] rsp0={:#x} not a plausible stack; skipping frame walk\n",
            rsp0,
        ));
        return;
    }
    let mut rbp = rbp0;
    let mut i = 0usize;
    while i < 24 && plausible(rbp) && (rbp & 0x7) == 0 {
        // SAFETY: rbp is a validated, 8-aligned kernel-range address; a
        // frame is [saved_rbp, return_addr]. If the chain is corrupt the
        // reads may fault, but we are already panicking.
        let saved_rbp = unsafe { core::ptr::read_volatile(rbp as *const u64) };
        let ret = unsafe { core::ptr::read_volatile((rbp + 8) as *const u64) };
        if ret < 0x1000 {
            kernel_hal::console::serial_write_fmt_spin(format_args!(
                "[kfault-bt]   #{:02} ret={:#x} (rbp={:#x}) — abort walk (corrupt frame)\n",
                i, ret, rbp,
            ));
            break;
        }
        kernel_hal::console::serial_write_fmt_spin(format_args!(
            "[kfault-bt]   #{:02} ret={:#x} (rbp={:#x})\n",
            i, ret, rbp,
        ));
        if saved_rbp <= rbp {
            break; // frame pointers must strictly increase up the stack
        }
        rbp = saved_rbp;
        i += 1;
    }
    // Also raw-scan the stack for kernel code pointers as a fallback when the
    // frame chain is broken (memset is a leaf that may omit rbp).
    kernel_hal::console::serial_write_fmt_spin(format_args!(
        "[kfault-bt] raw stack scan from rsp:\n",
    ));
    let mut sp = rsp0 & !0x7u64;
    // Wider bound for everything below: `sp` only ever advances by 8 bytes at
    // a time from `rsp0` (at most 4 KiB total across the whole scan loop), so
    // it stays local to a known-live pointer no matter how wide this bound
    // is -- unlike the rbp walk above, widening it doesn't risk chasing an
    // untrusted value far off into unmapped memory. `plausible(w)` below
    // never dereferences `w`, only reports it, so it's equally safe to use
    // here. See the top-of-function comment for why this needed widening at
    // all: two real captures showed a live rsp being rejected by the
    // original 256 MiB bound.
    let plausible_sp = |a: u64| a >= 0xffff_ff00_0000_0000 && a < 0xffff_ff01_0000_0000;
    // [diag] The single most reliable value here, but its interpretation
    // depends on the fault type:
    //   EXECUTE fault (indirect `call` through a null/corrupted fn-ptr):
    //     The CPU pushes the return address BEFORE loading the bad RIP, so
    //     [rsp0] = the return site of the bad call (names the caller).
    //   READ/WRITE fault (null dereference in kernel data path):
    //     No push occurs; [rsp0] = the return address of the *faulting*
    //     function itself. If this is a non-kernel value (e.g. 0x13406) the
    //     return-address slot has been overwritten — likely by a coroutine
    //     stack overflow that also corrupted a heap pointer to NULL.
    // Still gated on `plausible_sp(sp)` (the ADDRESS, not the value) though:
    // sp itself came from the trap frame, but if the underlying bug corrupts
    // more than just one fn-ptr, rsp could be garbage too, and dereferencing
    // an unmapped address here would fault again while already handling a
    // fault -- an #PF-during-#PF is one of the CPU's own double-fault
    // triggers (see the Double Fault this exact bug produced once already).
    if plausible_sp(sp) {
        let top = unsafe { core::ptr::read_volatile(sp as *const u64) };
        let label = if access_flags.contains(MMUFlags::EXECUTE) {
            "return address pushed by bad call (caller of the null fn-ptr)"
        } else {
            "return address of the faulting function (non-kernel value = stack corruption)"
        };
        kernel_hal::console::serial_write_fmt_spin(format_args!(
            "[kfault-bt]   [rsp0]={:#x} <- {}\n",
            top, label,
        ));
    } else {
        kernel_hal::console::serial_write_fmt_spin(format_args!(
            "[kfault-bt]   rsp0={:#x} is itself out of the plausible kernel range -- \
             not dereferencing it (avoids a #PF-during-#PF -> double fault)\n",
            sp,
        ));
    }
    let mut found = 0usize;
    let mut scanned = 0usize;
    while found < 20 && scanned < 512 && plausible_sp(sp) {
        let w = unsafe { core::ptr::read_volatile(sp as *const u64) };
        if plausible_sp(w) {
            kernel_hal::console::serial_write_fmt_spin(format_args!(
                "[kfault-bt]   @{:#x} = {:#x}\n",
                sp, w,
            ));
            found += 1;
        }
        sp += 8;
        scanned += 1;
    }
}

/// [diag] Walk every executor (try_lock) and print whether fault rsp/rbp sit on
/// a coroutine stack, in a guard VA, or outside all stacks.
#[cfg(not(feature = "libos"))]
fn report_soft_smash_stack_attr(rsp: usize, rbp: usize) {
    let attr = executor::attribute_fault_stack_ptrs(rsp, rbp);
    let fmt_hit = |label: &str, hit: Option<executor::StackAttrHit>| {
        match hit {
            Some(h) => kernel_hal::console::serial_write_fmt_spin(format_args!(
                "[soft-smash] {}={:#x} -> CPU{} exec={} task={} stack_base={:#x} region={}\n",
                label,
                if label == "rsp" { rsp } else { rbp },
                h.cpu,
                h.executor_id,
                h.task_id,
                h.stack_base,
                h.region.as_str(),
            )),
            None => kernel_hal::console::serial_write_fmt_spin(format_args!(
                "[soft-smash] {}={:#x} -> OUTSIDE all executor stacks \
                 (walked {} CPUs, skipped {}, {} executors)\n",
                label,
                if label == "rsp" { rsp } else { rbp },
                attr.cpus_walked,
                attr.cpus_skipped,
                attr.executors_seen,
            )),
        }
    };
    fmt_hit("rsp", attr.rsp);
    fmt_hit("rbp", attr.rbp);
}
