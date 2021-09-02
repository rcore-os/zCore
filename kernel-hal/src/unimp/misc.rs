use crate::{context::UserContext, VirtAddr};
use acpi::Acpi;

/// Get fault address of the last page fault.
pub fn fetch_fault_vaddr() -> VirtAddr {
    unimplemented!()
}

pub fn fetch_trap_num(_context: &UserContext) -> usize {
    unimplemented!()
}

/// Get physical address of `acpi_rsdp` and `smbios` on x86_64.
pub fn pc_firmware_tables() -> (u64, u64) {
    unimplemented!()
}

/// Get ACPI Table
pub fn get_acpi_table() -> Option<Acpi> {
    unimplemented!()
}

/// IO Ports access on x86 platform
pub fn outpd(_port: u16, _value: u32) {
    unimplemented!()
}

pub fn inpd(_port: u16) -> u32 {
    unimplemented!()
}

/// Get local APIC ID
pub fn apic_local_id() -> u8 {
    unimplemented!()
}
