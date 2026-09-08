//! Synchronous TLB invalidation, including CPUs waiting on an IRQ-disabled lock.
//!
//! Page table updates happen under such locks, so an ordinary maskable IPI can
//! deadlock against a CPU waiting for the same lock. The NMI handler below only
//! flushes the TLB and acknowledges a sequence number; it never enters Rust,
//! takes a lock, uses GS, or schedules a task.

use core::arch::global_asm;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::config_common::MAX_CORE_NUM;

#[unsafe(no_mangle)]
static ZCORE_TLB_REQUEST: [AtomicUsize; MAX_CORE_NUM] =
    [const { AtomicUsize::new(0) }; MAX_CORE_NUM];
#[unsafe(no_mangle)]
static ZCORE_TLB_ACK: [AtomicUsize; MAX_CORE_NUM] = [const { AtomicUsize::new(0) }; MAX_CORE_NUM];

global_asm!(
    r#"
    .text
    .balign 4
    .global zcore_tlb_nmi
zcore_tlb_nmi:
    push rax
    push rbx
    push rcx
    push rdx
    mov eax, 1
    xor ecx, ecx
    cpuid
    shr ebx, 24
    lea rax, [rip + ZCORE_TLB_REQUEST]
    mov rdx, [rax + rbx * 8]
    mov rcx, cr4
    mov rax, rcx
    and rax, -129
    mov cr4, rax
    mov cr4, rcx
    lea rax, [rip + ZCORE_TLB_ACK]
    mov [rax + rbx * 8], rdx
    pop rdx
    pop rcx
    pop rbx
    pop rax
    iretq
"#
);

pub(super) fn init() {
    use x86_64::{instructions::tables::sidt, structures::idt::InterruptDescriptorTable, VirtAddr};
    unsafe extern "C" {
        fn zcore_tlb_nmi();
    }
    // NMI may interrupt the syscall trampoline while RSP points into a saved
    // user context. Always use a dedicated IST stack rather than that RSP.
    #[repr(C, align(16))]
    struct NmiStack([u8; 16384]);
    let stack = alloc::boxed::Box::leak(alloc::boxed::Box::new(NmiStack([0; 16384])));
    // trapframe allocates a private writable IDT for this CPU. Install before
    // publishing it in ONLINE, so every shootdown target has a working handler.
    unsafe {
        let tss = &mut *x86_64::registers::model_specific::GsBase::read()
            .as_mut_ptr::<x86_64::structures::tss::TaskStateSegment>();
        tss.interrupt_stack_table[0] = VirtAddr::new(stack.0.as_ptr() as u64 + 16384);
        let idt = &mut *sidt().base.as_mut_ptr::<InterruptDescriptorTable>();
        idt.non_maskable_interrupt
            .set_handler_addr(VirtAddr::from_ptr(zcore_tlb_nmi as *const ()))
            .set_stack_index(0);
    }
}

pub(super) fn shootdown() {
    let current = super::cpu::cpu_id() as usize;
    let targets = super::smp::ONLINE.load(Ordering::Acquire) & !(1 << current);
    let mut requests = [0; MAX_CORE_NUM];
    for (id, request) in requests.iter_mut().enumerate() {
        if targets & (1 << id) != 0 {
            *request = ZCORE_TLB_REQUEST[id].fetch_add(1, Ordering::AcqRel) + 1;
            super::smp::send_ipi(id as u32, 4 << 8); // NMI delivery mode
        }
    }
    for (id, request) in requests.iter().enumerate() {
        while ZCORE_TLB_ACK[id].load(Ordering::Acquire) < *request {
            core::hint::spin_loop();
        }
    }
}
