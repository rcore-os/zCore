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

use super::consts::PHYSICAL_MEMORY_OFFSET;
use core::{
    arch::{asm, global_asm},
    str::FromStr,
};
use kernel_hal::{
    arch::sbi::{hart_start, send_ipi, SBI_SUCCESS},
    KernelConfig,
};

#[no_mangle]
pub extern "C" fn primary_rust_main(hartid: usize, device_tree_paddr: usize) -> ! {
    let sstatus: usize;
    unsafe { asm!("csrr {0}, sstatus", out(reg) sstatus) };
    println!(
        "boot hart: zCore rust_main(hartid: {}, device_tree_paddr: {:#x}) sstatus={:#x}",
        hartid, device_tree_paddr, sstatus
    );

    for id in 0..usize::from_str(core::env!("SMP")).expect("can't parse SMP as usize.") {
        if id != hartid {
            extern "C" {
                fn _secondary_hart_start();
            }
            let err_code = hart_start(
                id,
                _secondary_hart_start as usize - PHYSICAL_MEMORY_OFFSET, // cal physical address
                0,
            );
            if err_code != SBI_SUCCESS {
                panic!("start hart{} failed. error code={}", id, err_code);
            }
            let hart_mask = 1usize << id;
            let err_code = send_ipi(&hart_mask as *const _ as usize);
            if err_code != SBI_SUCCESS {
                panic!("send ipi to hart{} failed. error code={}", id, err_code);
            }
        }
    }

    crate::primary_main(KernelConfig {
        phys_to_virt_offset: PHYSICAL_MEMORY_OFFSET,
        dtb_paddr: device_tree_paddr,
    });
    unreachable!()
}

#[no_mangle]
pub extern "C" fn secondary_rust_main() -> ! {
    crate::secondary_main()
}
