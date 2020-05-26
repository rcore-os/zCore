use super::*;

#[derive(Default)]
pub struct Region {
    pub base: u64,
}

pub struct RegionAllocator {
}

impl RegionAllocator {
    pub fn get_region(&self, _addr: u64, _size: u64) -> ZxResult<Region> { Ok(Default::default())}
    // WARNING
}