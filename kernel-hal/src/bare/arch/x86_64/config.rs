/// Configuration of HAL.
#[derive(Debug)]
pub struct KernelConfig {
    pub kernel_offset: usize,
    pub phys_mem_start: usize,
    pub phys_to_virt_offset: usize,

    pub acpi_rsdp: u64,
    pub smbios: u64,
    pub ap_fn: fn() -> !,
}
