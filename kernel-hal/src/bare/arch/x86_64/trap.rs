#![allow(dead_code)]
#![allow(clippy::identity_op)]

use trapframe::TrapFrame;

// Reference: https://wiki.osdev.org/Exceptions
const DIVIDE_ERROR: usize = 0;
const DEBUG: usize = 1;
const NON_MASKABLE_INTERRUPT: usize = 2;
const BREAKPOINT: usize = 3;
const OVERFLOW: usize = 4;
const BOUND_RANGE_EXCEEDED: usize = 5;
const INVALID_OPCODE: usize = 6;
const DEVICE_NOT_AVAILABLE: usize = 7;
const DOUBLE_FAULT: usize = 8;
const COPROCESSOR_SEGMENT_OVERRUN: usize = 9;
const INVALID_TSS: usize = 10;
const SEGMENT_NOT_PRESENT: usize = 11;
const STACK_SEGMENT_FAULT: usize = 12;
const GENERAL_PROTECTION_FAULT: usize = 13;
const PAGE_FAULT: usize = 14;
const FLOATING_POINTEXCEPTION: usize = 16;
const ALIGNMENT_CHECK: usize = 17;
const MACHINE_CHECK: usize = 18;
const SIMD_FLOATING_POINT_EXCEPTION: usize = 19;
const VIRTUALIZATION_EXCEPTION: usize = 20;
const SECURITY_EXCEPTION: usize = 30;

// IRQ vectors
pub(super) const X86_INT_BASE: usize = 0x20;
pub(super) const X86_INT_MAX: usize = 0xff;

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

fn double_fault(tf: &TrapFrame) {
    panic!("\nEXCEPTION: Double Fault\n{:#x?}", tf);
}

fn page_fault(tf: &mut TrapFrame) {
    let (fault_vaddr, access_flags) = crate::context::fetch_page_fault_info(tf.error_code);
    crate::KHANDLER.handle_page_fault(fault_vaddr, access_flags);
}

#[no_mangle]
pub extern "C" fn trap_handler(tf: &mut TrapFrame) {
    trace!(
        "Interrupt: {:#x} @ CPU{}",
        tf.trap_num,
        super::cpu::cpu_id()
    );
    match tf.trap_num {
        BREAKPOINT => breakpoint(),
        DOUBLE_FAULT => double_fault(tf),
        PAGE_FAULT => page_fault(tf),
        X86_INT_BASE..=X86_INT_MAX => crate::interrupt::handle_irq(tf.trap_num),
        _ => panic!("Unhandled interrupt {:x} {:#x?}", tf.trap_num, tf),
    }
}
