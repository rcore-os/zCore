//! Various builders to probe devices and create corresponding drivers
//! (e.g. device tree, ACPI table, ...)

mod devicetree;

pub use devicetree::DevicetreeDriverBuilder;

use crate::{PhysAddr, VirtAddr};

/// A trait implemented in kernel to translate device physical addresses to virtual
/// addresses.
pub trait IoMapper {
    /// Translate the device physical address to virtual address. If not mapped
    /// in the kernel page table, map the region specified by the given `size`.
    ///
    /// If an error accurs during translation or mapping, returns `None`.
    fn query_or_map(&self, paddr: PhysAddr, size: usize) -> Option<VirtAddr>;
}
