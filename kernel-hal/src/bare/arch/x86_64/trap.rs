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
            crate::KHANDLER.handle_page_fault(vaddr, flags)
        }
        TrapReason::Interrupt(vector) => {
            // [diag] Attribute the scheduler tick to the context it interrupted:
            // ring 3 (CS low 2 bits == 3) means a user thread was running; ring 0
            // means kernel (idle hlt / syscall / kernel spin). Localises a pegged
            // core's busy time to user vs kernel for /proc/perf.
            if vector == X86_INT_APIC_TIMER {
                crate::kstats::note_tick_context(tf.cs & 0b11 == 0b11, tf.rip as u64);
                // [diag] Coroutine-stack overflow tripwire: the executor stacks
                // are guard-page-less heap allocations, so a runaway kernel
                // call chain corrupts the heap silently (the /proc/self/exe
                // recursion bug). Checking the current executor's base canary
                // every tick converts that into a labelled panic within ~4 ms.
                executor::check_current_executor_canary();
                // [diag] Debug registers are per-CPU and are not part of the
                // task context, so an armed watchpoint has to be programmed
                // on every core. Doing it here means each core picks up an
                // arm/disarm within one tick, with no IPI; it is a relaxed
                // load and a compare when nothing changed.
                crate::watchpoint::sync_this_cpu();
            }
            crate::interrupt::handle_irq(vector);
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
