use rboot::{GraphicInfo, MemoryDescriptor};

/// Configuration of HAL.
#[derive(Debug)]
pub struct KernelConfig {
    pub cmdline: &'static str,
    pub initrd_start: usize,
    pub initrd_size: usize,

    pub memory_map: &'static [&'static MemoryDescriptor],
    pub phys_to_virt_offset: usize,
    pub graphic_info: GraphicInfo,

    pub acpi_rsdp: u64,
    pub smbios: u64,
    pub ap_fn: fn() -> !,
}
