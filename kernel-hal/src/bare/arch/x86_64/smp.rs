//! Start only firmware-enumerated APs, with private stacks and an entry handshake.

use alloc::alloc::{alloc_zeroed, handle_alloc_error, Layout};
use core::arch::global_asm;
use core::sync::atomic::{AtomicUsize, Ordering};
use zcore_drivers::irq::x86::Apic;

use crate::{mem::phys_to_virt, KCONFIG};

global_asm!(include_str!("smp.S"));

const TRAMPOLINE: usize = 0x6000;
pub(super) static ONLINE: AtomicUsize = AtomicUsize::new(0);
static ENTERED: AtomicUsize = AtomicUsize::new(usize::MAX);

extern "C" fn ap_entry() {
    crate::boot::secondary_init();
    let id = super::cpu::cpu_id() as usize;
    ONLINE.fetch_or(1 << id, Ordering::Release);
    ENTERED.store(id, Ordering::Release);
    (KCONFIG.ap_fn)();
}

fn delay_us(us: u64) {
    let start = unsafe { core::arch::x86_64::_rdtsc() };
    let ticks = u64::from(super::cpu::cpu_frequency()) * us;
    while unsafe { core::arch::x86_64::_rdtsc() }.wrapping_sub(start) < ticks {
        core::hint::spin_loop();
    }
}

pub(super) fn send_ipi(destination: u32, command: u32) {
    unsafe {
        let base = x86::msr::rdmsr(0x1b);
        if base & (1 << 10) != 0 {
            x86::msr::wrmsr(0x830, (u64::from(destination) << 32) | u64::from(command));
        } else {
            assert!(destination < 256);
            let registers = phys_to_virt(base as usize & 0xffff_f000);
            let low = (registers + 0x300) as *mut u32;
            let high = (registers + 0x310) as *mut u32;
            high.write_volatile(destination << 24);
            low.write_volatile(command);
            for _ in 0..1000 {
                if low.read_volatile() & (1 << 12) == 0 {
                    return;
                }
                delay_us(10);
            }
            panic!("APIC IPI delivery timed out for CPU {}", destination);
        }
    }
}

pub(super) fn start() {
    let limit = option_env!("SMP")
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(crate::config_common::MAX_CORE_NUM);
    let bsp = super::cpu::cpu_id() as u32;
    assert!((bsp as usize) < crate::config_common::MAX_CORE_NUM);
    assert!(limit <= crate::config_common::MAX_CORE_NUM);
    ONLINE.store(1 << bsp, Ordering::Release);
    let ids = Apic::application_processor_ids(KCONFIG.acpi_rsdp as usize, phys_to_virt);
    info!(
        "SMP: BSP APIC {}, available APs {:?}, limit {}",
        bsp, ids, limit
    );
    info!("SMP: IA32_APIC_BASE={:#x}", unsafe {
        x86::msr::rdmsr(0x1b)
    });
    if limit <= 1 || ids.is_empty() {
        return;
    }
    unsafe {
        unsafe extern "C" {
            fn zcore_ap_start();
            fn zcore_ap_end();
        }
        let size = zcore_ap_end as *const () as usize - zcore_ap_start as *const () as usize;
        assert!(size < 0xfe0);
        core::ptr::copy_nonoverlapping(
            zcore_ap_start as *const u8,
            phys_to_virt(TRAMPOLINE) as *mut u8,
            size,
        );
        let cr3 = x86::controlregs::cr3();
        assert!(cr3 <= u32::MAX as u64);
        (phys_to_virt(0x6ff8) as *mut u32).write(cr3 as u32);
        (phys_to_virt(0x6ff0) as *mut usize).write(ap_entry as *const () as usize);
        for id in ids.into_iter().filter(|id| *id != bsp).take(limit - 1) {
            assert!(
                (id as usize) < crate::config_common::MAX_CORE_NUM,
                "unsupported APIC ID {}",
                id
            );
            let layout = Layout::from_size_align(64 * 1024, 16).unwrap();
            let stack = alloc_zeroed(layout);
            if stack.is_null() {
                handle_alloc_error(layout);
            }
            // AP stacks live for the lifetime of the kernel.
            (phys_to_virt(0x6fe8) as *mut usize).write(stack as usize + layout.size());
            ENTERED.store(usize::MAX, Ordering::Release);
            // Use the active APIC mode for both the destination encoding and
            // register access; x2APIC ICR writes must go through the MSR.
            send_ipi(id, 0x4500);
            delay_us(10_000);
            send_ipi(id, 0x4600 | (TRAMPOLINE >> 12) as u32);
            delay_us(200);
            if ENTERED.load(Ordering::Acquire) != id as usize {
                send_ipi(id, 0x4600 | (TRAMPOLINE >> 12) as u32);
            }
            for _ in 0..1000 {
                if ENTERED.load(Ordering::Acquire) == id as usize {
                    break;
                }
                delay_us(1000);
            }
            assert_eq!(
                ENTERED.load(Ordering::Acquire),
                id as usize,
                "AP {} failed to start",
                id
            );
            info!("SMP: AP {} entered kernel", id);
        }
    }
}
