//! Functions only available on x86 platforms.

pub use zcore_drivers::io::{Io, Pio};

/// Get physical address of `acpi_rsdp` and `smbios` on x86_64.
pub fn pc_firmware_tables() -> (u64, u64) {
    (crate::KCONFIG.acpi_rsdp, crate::KCONFIG.smbios)
}
