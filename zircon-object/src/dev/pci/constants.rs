#![allow(missing_docs)]
use super::*;

pub const PCIE_PIO_ADDR_SPACE_MASK: u64 = 0xFFFF_FFFF;
pub const PCIE_MAX_BUSSES: usize = 256;
pub const PCIE_ECAM_BYTES_PER_BUS: usize =
    4096 * PCI_MAX_DEVICES_PER_BUS * PCI_MAX_FUNCTIONS_PER_DEVICE;
pub const PCIE_INVALID_VENDOR_ID: usize = 0xFFFF;

pub const PCIE_IRQRET_MASK: u32 = 0x1;
pub const PCIE_MAX_MSI_IRQS: u32 = 32;
