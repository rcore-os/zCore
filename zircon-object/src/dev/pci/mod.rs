// #![deny(missing_docs)]
// use super::*;
mod bus;
mod caps;
mod config;
mod constants;
mod nodes;
pub mod pci_init_args;
mod pio;

use super::*;
use alloc::sync::*;
pub(crate) use nodes::*;
use pci_init_args::*;
use pio::*;

pub use self::bus::{
    MmioPcieAddressProvider, PCIeBusDriver, PcieDeviceInfo, PcieDeviceKObject,
    PioPcieAddressProvider,
};
pub use self::constants::*;
pub use self::nodes::PcieIrqMode;
pub use self::pio::{pio_config_read, pio_config_write};

/// Type of PCI address space.
#[derive(PartialEq, Debug)]
pub enum PciAddrSpace {
    /// Memory mapping I/O.
    MMIO,
    /// Port I/O.
    PIO,
}

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

pub struct MappedEcamRegion {
    pub ecam: PciEcamRegion,
    pub vaddr: u64,
}
