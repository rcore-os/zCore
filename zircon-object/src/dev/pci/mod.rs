#![allow(missing_docs)]

mod bus;
mod caps;
mod config;
mod nodes;
pub mod pci_init_args;
mod pio;

pub use self::bus::{
    MmioPcieAddressProvider, PCIeBusDriver, PcieDeviceInfo, PcieDeviceKObject,
    PioPcieAddressProvider,
};
pub use self::nodes::{IPciNode, PcieIrqMode};
pub use self::pio::{pio_config_read, pio_config_write};

/// Type of PCI address space.
#[derive(PartialEq, Debug)]
#[allow(clippy::upper_case_acronyms)]
pub enum PciAddrSpace {
    /// Memory mapping I/O.
    MMIO,
    /// Port I/O.
    PIO,
}

/// ECAM Region.
pub struct PciEcamRegion {
    /// Physical address of the memory mapped config region.
    pub phys_base: u64,
    /// Size (in bytes) of the memory mapped config region.
    pub size: usize,
    /// Inclusive ID of the first bus controlled by this region.
    pub bus_start: u8,
    /// Inclusive ID of the last bus controlled by this region.
    pub bus_end: u8,
}

/// Mapped ECAM Region.
pub struct MappedEcamRegion {
    ecam: PciEcamRegion,
    vaddr: u64,
}

#[allow(missing_docs)]
pub mod constants {
    pub const PCI_MAX_DEVICES_PER_BUS: usize = 32;
    pub const PCI_MAX_FUNCTIONS_PER_DEVICE: usize = 8;
    pub const PCI_MAX_LEGACY_IRQ_PINS: usize = 4;
    pub const PCI_MAX_FUNCTIONS_PER_BUS: usize =
        PCI_MAX_FUNCTIONS_PER_DEVICE * PCI_MAX_DEVICES_PER_BUS;
    pub const PCI_MAX_IRQS: usize = 224;

    pub const PCI_NO_IRQ_MAPPING: u32 = u32::MAX;
    pub const PCIE_PIO_ADDR_SPACE_MASK: u64 = 0xFFFF_FFFF;
    pub const PCIE_MAX_BUSSES: usize = 256;
    pub const PCIE_ECAM_BYTES_PER_BUS: usize =
        4096 * PCI_MAX_DEVICES_PER_BUS * PCI_MAX_FUNCTIONS_PER_DEVICE;
    pub const PCIE_INVALID_VENDOR_ID: usize = 0xFFFF;

    pub const PCI_CFG_SPACE_TYPE_PIO: u8 = 0;
    pub const PCI_CFG_SPACE_TYPE_MMIO: u8 = 1;
    pub const PCIE_IRQRET_MASK: u32 = 0x1;
    pub const PCIE_MAX_MSI_IRQS: usize = 32;
}
