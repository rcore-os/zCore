global_asm!(include_str!("boot.asm"));

use core::arch::{asm, global_asm};
use core::str::FromStr;
use kernel_hal::arch::sbi::{hart_start, send_ipi, SBI_SUCCESS};
use kernel_hal::KernelConfig;

pub mod consts {
    pub const KERNEL_OFFSET: usize = 0xFFFF_FFFF_8000_0000;
    pub const PHYS_MEMORY_BASE: usize = 0x8000_0000;
    pub const PHYSICAL_MEMORY_OFFSET: usize = KERNEL_OFFSET - PHYS_MEMORY_BASE;
    pub const KERNEL_HEAP_SIZE: usize = 80 * 1024 * 1024;

    /// Get HART number from the environment variable
    pub const SMP: &str = core::env!("SMP");

    #[inline]
    pub fn phys_memory_base() -> usize {
        PHYS_MEMORY_BASE
    }
}

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
            "\nzCore rust_main(hartid: {}, device_tree_paddr: {:#x}) sstatus={:#x}\n",
            hartid, device_tree_paddr, sstatus
        );
    };

    for id in 0..usize::from_str(consts::SMP).expect("can't parse SMP as usize.") {
        #[cfg(feature = "board-fu740")]
        if id == 0 {
            continue;
        }

        if id != hartid {
            println!("hart{id} is booting");
            let err_code = hart_start(
                id,
                secondary_hart_start as usize - consts::PHYSICAL_MEMORY_OFFSET, // cal physical address
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
        } else {
            println!("hart{id} is the primary hart");
        }
    }

    let config = KernelConfig {
        phys_to_virt_offset: consts::PHYSICAL_MEMORY_OFFSET,
        dtb_paddr: device_tree_paddr,
    };
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
