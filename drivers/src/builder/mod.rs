mod dt;

pub use dt::DeviceTreeDriverBuilder;

use crate::{PhysAddr, VirtAddr};

pub trait IoMapper {
    fn query_or_map(&self, paddr: PhysAddr, size: usize) -> Option<VirtAddr>;
}
