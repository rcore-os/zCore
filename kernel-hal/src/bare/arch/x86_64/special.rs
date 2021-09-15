use x86_64::instructions::port::Port;

/// IO Port in instruction
pub fn pio_read(port: u16) -> u32 {
    unsafe { Port::new(port).read() }
}

/// IO Port out instruction
pub fn pio_write(port: u16, value: u32) {
    unsafe { Port::new(port).write(value) }
}

/// Get physical address of `acpi_rsdp` and `smbios` on x86_64.
pub fn pc_firmware_tables() -> (u64, u64) {
    (crate::KCONFIG.acpi_rsdp, crate::KCONFIG.smbios)
}
