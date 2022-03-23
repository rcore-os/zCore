#[cfg(feature = "board-qemu")]
global_asm!(
    include_str!("boot/boot_qemu.asm"),
    include_str!("boot/entry64.asm"),
);

#[cfg(feature = "board-d1")]
global_asm!(
    include_str!("boot/boot_d1.asm"),
    include_str!("boot/entry64.asm"),
);

use super::consts::*;
use core::arch::{asm, global_asm};
use core::str::FromStr;
use kernel_hal::arch::sbi::{hart_start, send_ipi, SBI_SUCCESS};
use kernel_hal::KernelConfig;
const SMP: &str = core::env!("SMP"); // Get HART number from the environment variable

extern "C" {
    fn secondary_hart_start();
}

#[no_mangle]
pub extern "C" fn primary_rust_main(hartid: usize, device_tree_paddr: usize) -> ! {
    unsafe {
        asm!("mv tp, {0}", in(reg) hartid);
        let mut sstatus: usize;
        asm!("csrr {0}, sstatus", out(reg) sstatus);
        sstatus |= 1 << 18;
        asm!("csrw sstatus, {0}", in(reg) sstatus);
        println!(
            "boot hart: zCore rust_main(hartid: {}, device_tree_paddr: {:#x}) sstatus={:x}",
            hartid, device_tree_paddr, sstatus
        );
    };

    let config = KernelConfig {
        phys_to_virt_offset: PHYSICAL_MEMORY_OFFSET,
        dtb_paddr: device_tree_paddr,
    };
    for id in 0..usize::from_str(SMP).expect("can't parse SMP as usize.") {
        if id != hartid {
            let err_code = hart_start(
                id,
                secondary_hart_start as usize - PHYSICAL_MEMORY_OFFSET, // cal physical address
                0,
            );
            if err_code != SBI_SUCCESS {
                panic!("start hart{} failed. error code={}", id, err_code);
            }
            let hart_mask: usize = 1 << id;
            let err_code = send_ipi(&hart_mask as *const _ as usize);
            if err_code != SBI_SUCCESS {
                panic!("send ipi to hart{} failed. error code={}", id, err_code);
            }
        }
    }
    crate::primary_main(config);
    unreachable!()
}

// Don't print in this function and use console_write_early if necessary
#[no_mangle]
pub extern "C" fn secondary_rust_main(hartid: usize) -> ! {
    unsafe {
        asm!("mv tp, {0}", in(reg) hartid);
        let mut sstatus: usize;
        asm!("csrr {0}, sstatus", out(reg) sstatus);
        sstatus |= 1 << 18; // set SUM=1
        asm!("csrw sstatus, {0}", in(reg) sstatus);
    };
    crate::secondary_main();
}
