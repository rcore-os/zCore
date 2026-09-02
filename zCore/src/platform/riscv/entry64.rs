global_asm!(include_str!("boot.asm"));

use core::arch::{asm, global_asm};
use core::str::FromStr;
use kernel_hal::arch::sbi::{hart_start, send_ipi, SBI_SUCCESS};
use kernel_hal::KernelConfig;

#[no_mangle]
pub static PHY_MEM_OFS: usize = consts::KERNEL_BASE - consts::PHYS_MEMORY_BASE;

pub mod consts {
    cfg_if! {
        if #[cfg(feature = "board-fu740")] {
            pub const KERNEL_BASE: usize = 0xFFFF_FFE0_8000_0000;
            pub const PHYS_MEMORY_BASE: usize = 0x8000_0000;
        } else if #[cfg(feature = "board-c910light")] {
            pub const KERNEL_BASE: usize = 0xffffffe0_00200000;
            pub const PHYS_MEMORY_BASE: usize = 0x200000;
        }
    }
    #[allow(dead_code)]
    pub const KERNEL_HEAP_SIZE: usize = 80 * 1024 * 1024;
    /// Get HART number from the environment variable
    pub const SMP: &str = core::env!("SMP");

    #[inline]
    pub fn phys_to_virt_offset() -> usize {
        KERNEL_BASE - PHYS_MEMORY_BASE
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

    println!("      ____");
    println!(" ____/ ___|___  _ __ ___");
    println!("|_  / |   / _ \\| '__/ _ \\");
    println!(" / /| |__| (_) | | |  __/");
    println!("/___|\\____\\___/|_|  \\___|");
    println!();

    for id in 0..usize::from_str(consts::SMP).expect("can't parse SMP as usize.") {
        #[cfg(feature = "board-fu740")]
        if id == 0 {
            continue;
        }

        if id != hartid {
            println!("hart{id} is booting");
            let err_code = hart_start(
                id,
                secondary_hart_start as usize - PHY_MEM_OFS, // cal physical address
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
        phys_to_virt_offset: PHY_MEM_OFS,
        dtb_paddr: device_tree_paddr,
        dtb_size: 2 * 1024 * 1024,
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
