#[cfg(feature = "board-qemu")]
global_asm!(include_str!("boot/boot_qemu.asm"));
#[cfg(feature = "board-d1")]
global_asm!(include_str!("boot/boot_d1.asm"));

global_asm!(include_str!("boot/entry64.asm"));

use super::consts::*;
use kernel_hal::KernelConfig;

#[no_mangle]
pub extern "C" fn rust_main(hartid: usize, device_tree_paddr: usize) -> ! {
    println!(
        "zCore rust_main(hartid: {}, device_tree_paddr: {:#x})",
        hartid, device_tree_paddr
    );
    let config = KernelConfig {
        phys_to_virt_offset: PHYSICAL_MEMORY_OFFSET,
        dtb_paddr: device_tree_paddr,
    };
    crate::primary_main(config);
    unreachable!()
}
