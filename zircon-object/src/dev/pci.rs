// use super::*;
mod bus;

pub use bus::*;

#[derive(PartialEq)]
pub enum PciAddrSpace {
    MMIO,
    PIO,
}

pub const PCIE_PIO_ADDR_SPACE_MASK: u64 = 0xFFFFFFFF; // (1 << 32) - 1
