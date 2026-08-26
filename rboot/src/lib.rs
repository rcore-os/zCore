#![no_std]
#![deny(warnings)]

extern crate alloc;

use alloc::vec::Vec;
pub use uefi::proto::console::gop::ModeInfo;
pub use uefi::table::boot::{MemoryAttribute, MemoryDescriptor, MemoryType};

/// This structure represents the information that the bootloader passes to the kernel.
#[repr(C)]
#[derive(Debug)]
pub struct BootInfo {
    /// Referencias al buffer del mapa de memoria (vida `'static` vía `Box::leak` en main).
    pub memory_map: Vec<&'static MemoryDescriptor>,
    /// The offset into the virtual address space where the physical memory is mapped.
    pub physical_memory_offset: u64,
    /// The graphic output information
    pub graphic_info: GraphicInfo,
    /// Physical address of ACPI2 RSDP
    pub acpi2_rsdp_addr: u64,
    /// Physical address of SMBIOS
    pub smbios_addr: u64,
    /// The start physical address of initramfs
    pub initramfs_addr: u64,
    /// The size of initramfs
    pub initramfs_size: u64,
    /// Kernel command line
    pub cmdline: &'static str,
    /// Raw EDID (first 128-byte block) of the active display, read from the
    /// UEFI `EFI_EDID_ACTIVE_PROTOCOL` at boot. `edid_size` is 0 when the
    /// firmware exposed no EDID. Kept LAST so growing it never shifts any
    /// existing field's offset (ABI-stable across partial rebuilds).
    pub edid: [u8; 128],
    pub edid_size: u32,
}

/// Graphic output information
#[derive(Debug, Copy, Clone)]
#[repr(C)]
pub struct GraphicInfo {
    pub mode: ModeInfo,
    pub fb_addr: u64,
    pub fb_size: u64,
}
