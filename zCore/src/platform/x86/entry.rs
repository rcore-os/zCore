use kernel_hal::KernelConfig;
use rboot::BootInfo;

#[no_mangle]
pub extern "C" fn _start(boot_info: &'static BootInfo) -> ! {
    let info = boot_info.graphic_info;
    let config = KernelConfig {
        cmdline: boot_info.cmdline,
        initrd_start: boot_info.initramfs_addr,
        initrd_size: boot_info.initramfs_size,

        memory_map: &boot_info.memory_map,
        phys_to_virt_offset: boot_info.physical_memory_offset as _,

        fb_mode: info.mode,
        fb_addr: info.fb_addr,
        fb_size: info.fb_size,

        acpi_rsdp: boot_info.acpi2_rsdp_addr,
        smbios: boot_info.smbios_addr,
        ap_fn: crate::secondary_main,
    };
    crate::primary_main(config);
    unreachable!()
}
