#![allow(missing_docs)]
use super::*;

pub(super) const PCI_MAX_DEVICES_PER_BUS: usize = 32;
pub(super) const PCI_MAX_FUNCTIONS_PER_DEVICE: usize = 8;
pub(super) const PCI_MAX_LEGACY_IRQ_PINS: usize = 4;
pub(super) const PCI_MAX_FUNCTIONS_PER_BUS: usize =
    PCI_MAX_FUNCTIONS_PER_DEVICE * PCI_MAX_DEVICES_PER_BUS;
pub(super) const PCI_MAX_IRQS: usize = 224;
pub(super) const PCI_INIT_ARG_MAX_ECAM_WINDOWS: usize = 2;

pub const PCI_INIT_ARG_MAX_SIZE: usize = core::mem::size_of::<PciInitArgsAddrWindows>()
    * PCI_INIT_ARG_MAX_ECAM_WINDOWS
    + core::mem::size_of::<PciInitArgsHeader>();
pub const PCI_NO_IRQ_MAPPING: u32 = u32::MAX;
pub const PCIE_PIO_ADDR_SPACE_MASK: u64 = 0xFFFF_FFFF;
pub const PCIE_MAX_BUSSES: usize = 256;
pub const PCIE_ECAM_BYTES_PER_BUS: usize =
    4096 * PCI_MAX_DEVICES_PER_BUS * PCI_MAX_FUNCTIONS_PER_DEVICE;
pub const PCIE_INVALID_VENDOR_ID: usize = 0xFFFF;

pub const PCI_CFG_SPACE_TYPE_PIO: u8 = 0;
pub const PCI_CFG_SPACE_TYPE_MMIO: u8 = 1;
pub const PCIE_IRQRET_MASK: u32 = 0x1;
pub const PCIE_MAX_MSI_IRQS: u32 = 32;
