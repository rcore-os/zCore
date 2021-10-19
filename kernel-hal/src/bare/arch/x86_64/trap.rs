#![allow(dead_code)]

use trapframe::TrapFrame;

pub mod consts {
    // Reference: https://wiki.osdev.org/Exceptions
    pub const DIVIDE_ERROR: usize = 0;
    pub const DEBUG: usize = 1;
    pub const NON_MASKABLE_INTERRUPT: usize = 2;
    pub const BREAKPOINT: usize = 3;
    pub const OVERFLOW: usize = 4;
    pub const BOUND_RANGE_EXCEEDED: usize = 5;
    pub const INVALID_OPCODE: usize = 6;
    pub const DEVICE_NOT_AVAILABLE: usize = 7;
    pub const DOUBLE_FAULT: usize = 8;
    pub const COPROCESSOR_SEGMENT_OVERRUN: usize = 9;
    pub const INVALID_TSS: usize = 10;
    pub const SEGMENT_NOT_PRESENT: usize = 11;
    pub const STACK_SEGMENT_FAULT: usize = 12;
    pub const GENERAL_PROTECTION_FAULT: usize = 13;
    pub const PAGE_FAULT: usize = 14;
    pub const FLOATING_POINTEXCEPTION: usize = 16;
    pub const ALIGNMENT_CHECK: usize = 17;
    pub const MACHINE_CHECK: usize = 18;
    pub const SIMD_FLOATING_POINT_EXCEPTION: usize = 19;
    pub const VIRTUALIZATION_EXCEPTION: usize = 20;
    pub const SECURITY_EXCEPTION: usize = 30;

    // IRQ vectors
    pub const X86_INT_BASE: usize = 0x20;
    pub const X86_INT_MAX: usize = 0xff;

    pub const X86_INT_LOCAL_APIC_BASE: usize = 0xf0;
    pub const X86_INT_APIC_SPURIOUS: usize = X86_INT_LOCAL_APIC_BASE + 0x0;
    pub const X86_INT_APIC_TIMER: usize = X86_INT_LOCAL_APIC_BASE + 0x1;
    pub const X86_INT_APIC_ERROR: usize = X86_INT_LOCAL_APIC_BASE + 0x2;

    // ISA IRQ numbers
    pub const X86_ISA_IRQ_PIT: usize = 0;
    pub const X86_ISA_IRQ_KEYBOARD: usize = 1;
    pub const X86_ISA_IRQ_PIC2: usize = 2;
    pub const X86_ISA_IRQ_COM2: usize = 3;
    pub const X86_ISA_IRQ_COM1: usize = 4;
    pub const X86_ISA_IRQ_CMOSRTC: usize = 8;
    pub const X86_ISA_IRQ_MOUSE: usize = 12;
    pub const X86_ISA_IRQ_IDE: usize = 14;
}

pub use consts::*;

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
