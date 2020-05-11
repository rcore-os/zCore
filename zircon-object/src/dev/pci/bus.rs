use super::super::*;
use super::*;
use core::cmp::min;
use lazy_static::*;
use region_alloc::RegionAllocator;
use spin::Mutex;

pub struct PCIeBusDriver {
    mmio_lo: RegionAllocator,
    mmio_hi: RegionAllocator,
    pio_region: RegionAllocator,
}

lazy_static! {
    static ref _INSTANCE: Mutex<PCIeBusDriver> = Mutex::new(PCIeBusDriver::new());
}

impl PCIeBusDriver {
    pub fn add_bus_region(base: u64, size: u64, aspace: PciAddrSpace) -> ZxResult {
        _INSTANCE.lock().add_bus_region_inner(base, size, aspace)
    }
    pub fn sub_bus_region(base: u64, size: u64, aspace: PciAddrSpace) -> ZxResult {
        _INSTANCE.lock().sub_bus_region_inner(base, size, aspace)
    }
}

impl PCIeBusDriver {
    fn new() -> Self {
        PCIeBusDriver {
            mmio_lo: RegionAllocator::new(),
            mmio_hi: RegionAllocator::new(),
            pio_region: RegionAllocator::new(),
        }
    }
    pub fn add_bus_region_inner(&mut self, base: u64, size: u64, aspace: PciAddrSpace) -> ZxResult {
        self.add_or_sub_bus_region(base, size, aspace, true)
    }
    pub fn sub_bus_region_inner(&mut self, base: u64, size: u64, aspace: PciAddrSpace) -> ZxResult {
        self.add_or_sub_bus_region(base, size, aspace, false)
    }
    fn add_or_sub_bus_region(
        &mut self,
        base: u64,
        size: u64,
        aspace: PciAddrSpace,
        is_add: bool,
    ) -> ZxResult {
        if self.is_started(true) {
            return Err(ZxError::BAD_STATE);
        }
        if size == 0 {
            return Err(ZxError::INVALID_ARGS);
        }
        if aspace == PciAddrSpace::MMIO {
            let u32_max: u64 = u32::MAX as u64;
            let end = base + size;
            if base <= u32_max {
                let lo_size = min(u32_max + 1 - base, size);
                Self::add_sub_region_helper(is_add, &mut self.mmio_lo, base, lo_size);
            }
            if end > u32_max + 1 {
                let hi_size = min(end - (u32_max + 1), size);
                Self::add_sub_region_helper(is_add, &mut self.mmio_hi, end - hi_size, end);
            }
        } else if aspace == PciAddrSpace::PIO {
            let end = base + size - 1;
            if ((base | end) & !PCIE_PIO_ADDR_SPACE_MASK) != 0 {
                return Err(ZxError::INVALID_ARGS);
            }
            Self::add_sub_region_helper(is_add, &mut self.pio_region, base, size);
        }
        Ok(())
    }
    fn add_sub_region_helper(is_add: bool, region: &mut RegionAllocator, base: u64, size: u64) {
        if is_add {
            region.add(base as usize, size as usize)
        } else {
            region.subtract(base as usize, size as usize)
        }
    }
    fn is_started(&self, _allow_quirks_phase: bool) -> bool {
        false
    }
}
