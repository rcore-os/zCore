use kernel_hal::KernelConfig;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let config = KernelConfig {
        rt_services_addr: 0,
        rsdp_addr: 0,
        phys_to_virt_offset: 0xffff_0000_0000_0000,
    };
    crate::primary_main(config);
    unreachable!()
}
