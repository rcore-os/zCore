// use super::*;
mod bus;
mod pio;

pub use bus::*;
pub use pio::*;

#[derive(PartialEq)]
pub enum PciAddrSpace {
    MMIO,
    PIO,
}

pub const PCIE_PIO_ADDR_SPACE_MASK: u64 = 0xFFFFFFFF; // (1 << 32) - 1
