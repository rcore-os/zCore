use uefi::proto::console::gop::ModeInfo;
use uefi::table::boot::MemoryDescriptor;

/// Configuration of HAL.
#[derive(Debug)]
pub struct KernelConfig {
    pub cmdline: &'static str,
    pub initrd_start: u64,
    pub initrd_size: u64,

    pub memory_map: &'static [&'static MemoryDescriptor],
    pub phys_to_virt_offset: usize,

    pub fb_mode: ModeInfo,
    pub fb_addr: u64,
    pub fb_size: u64,

    pub acpi_rsdp: u64,
    pub smbios: u64,
    pub ap_fn: fn() -> !,
}
