#![allow(dead_code)]
#![allow(clippy::identity_op)]

use trapframe::TrapFrame;

use crate::context::TrapReason;

pub(super) const X86_INT_LOCAL_APIC_BASE: usize = 0xf0;
pub(super) const X86_INT_APIC_SPURIOUS: usize = X86_INT_LOCAL_APIC_BASE + 0x0;
pub(super) const X86_INT_APIC_TIMER: usize = X86_INT_LOCAL_APIC_BASE + 0x1;
pub(super) const X86_INT_APIC_ERROR: usize = X86_INT_LOCAL_APIC_BASE + 0x2;

// ISA IRQ numbers
pub(super) const X86_ISA_IRQ_PIT: usize = 0;
pub(super) const X86_ISA_IRQ_KEYBOARD: usize = 1;
pub(super) const X86_ISA_IRQ_PIC2: usize = 2;
pub(super) const X86_ISA_IRQ_COM2: usize = 3;
pub(super) const X86_ISA_IRQ_COM1: usize = 4;
pub(super) const X86_ISA_IRQ_CMOSRTC: usize = 8;
pub(super) const X86_ISA_IRQ_MOUSE: usize = 12;
pub(super) const X86_ISA_IRQ_IDE: usize = 14;

fn breakpoint() {
    panic!("\nEXCEPTION: Breakpoint");
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

    match TrapReason::from(tf.trap_num, tf.error_code) {
        TrapReason::HardwareBreakpoint | TrapReason::SoftwareBreakpoint => breakpoint(),
        TrapReason::PageFault(vaddr, flags) => crate::KHANDLER.handle_page_fault(vaddr, flags),
        TrapReason::Interrupt(vector) => {
            crate::interrupt::handle_irq(vector);
            if vector == X86_INT_APIC_TIMER {
                executor::handle_timeout();
            }
        }
        other => panic!("Unhandled trap {:x?} {:#x?}", other, tf),
    }
}
