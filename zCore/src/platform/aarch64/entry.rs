use super::consts::save_offset;
use kernel_hal::KernelConfig;
use rayboot::Aarch64BootInfo;
core::arch::global_asm!(include_str!("space.s"));

#[naked]
#[no_mangle]
#[link_section = ".text.entry"]
unsafe extern "C" fn _start() -> ! {
    core::arch::asm!(
        "
        adrp    x19, boot_stack_top
        mov     sp, x19
        b rust_main",
        options(noreturn),
    )
}

#[no_mangle]
extern "C" fn rust_main(boot_info: &'static Aarch64BootInfo) -> ! {
    let config = KernelConfig {
        cmdline: boot_info.cmdline,
        firmware_type: boot_info.firmware_type,
        uart_base: boot_info.uart_base,
        gic_base: boot_info.gic_base,
        phys_to_virt_offset: boot_info.offset,
    };
    save_offset(boot_info.offset);
    crate::primary_main(config);
    unreachable!()
}
