mod devicetree;

pub use devicetree::DevicetreeDriverBuilder;

use crate::{PhysAddr, VirtAddr};

pub trait IoMapper {
    fn query_or_map(&self, paddr: PhysAddr, size: usize) -> Option<VirtAddr>;
}
