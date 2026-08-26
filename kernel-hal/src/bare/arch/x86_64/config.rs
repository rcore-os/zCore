//! Kernel configuration.

use uefi::proto::console::gop::ModeInfo;
use uefi::table::boot::MemoryDescriptor;

/// Kernel configuration passed by kernel when calls [`crate::primary_init_early()`].
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
    /// Raw EDID (first block) of the active display from the UEFI bootloader;
    /// `fb_edid_size` is 0 when the firmware exposed none.
    pub fb_edid: [u8; 128],
    pub fb_edid_size: u32,

    pub acpi_rsdp: u64,
    pub smbios: u64,
    pub ap_fn: fn() -> !,
}
