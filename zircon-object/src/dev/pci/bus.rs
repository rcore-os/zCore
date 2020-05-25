use super::super::*;
use super::config::*;
use super::*;
use crate::vm::{kernel_allocate_physical, CachePolicy, MMUFlags, PhysAddr, VirtAddr};
use alloc::{collections::BTreeMap, sync::Arc, vec::Vec};
use core::cmp::min;
use core::marker::{Send, Sync};
use lazy_static::*;
use region_alloc::RegionAllocator;
use spin::Mutex;

pub struct PCIeBusDriver {
    mmio_lo: RegionAllocator,
    mmio_hi: RegionAllocator,
    pio_region: RegionAllocator,
    address_provider: Option<Arc<dyn PCIeAddressProvider + Sync + Send>>,
    roots: BTreeMap<usize, PciRoot>,
    state: PCIeBusDriverState,
    bus_topology: Mutex<()>,
    configs: Mutex<Vec<Arc<PciConfig>>>,
    legacy_irq_list: Mutex<Vec<Arc<SharedLegacyIrqHandler>>>,
}

#[derive(PartialEq)]
enum PCIeBusDriverState {
    NotStarted,
    StartingScanning,
    StartingRunningQuirks,
    StartingResourceAllocation,
    Operational,
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
    pub fn set_address_translation_provider(
        provider: Arc<dyn PCIeAddressProvider + Sync + Send>,
    ) -> ZxResult {
        _INSTANCE
            .lock()
            .set_address_translation_provider_inner(provider)
    }
    pub fn add_root(root: PciRoot) -> ZxResult {
        _INSTANCE.lock().add_root_inner(root)
    }
    pub fn start_bus_driver() -> ZxResult {
        _INSTANCE.lock().start_bus_driver_inner()
    }
}

impl PCIeBusDriver {
    fn new() -> Self {
        PCIeBusDriver {
            mmio_lo: RegionAllocator::new(),
            mmio_hi: RegionAllocator::new(),
            pio_region: RegionAllocator::new(),
            address_provider: None,
            roots: BTreeMap::new(),
            state: PCIeBusDriverState::NotStarted,
            bus_topology: Mutex::default(),
            legacy_irq_list: Mutex::new(Vec::new()),
            configs: Mutex::new(Vec::new()),
        }
    }
    pub fn add_bus_region_inner(&mut self, base: u64, size: u64, aspace: PciAddrSpace) -> ZxResult {
        self.add_or_sub_bus_region(base, size, aspace, true)
    }
    pub fn sub_bus_region_inner(&mut self, base: u64, size: u64, aspace: PciAddrSpace) -> ZxResult {
        self.add_or_sub_bus_region(base, size, aspace, false)
    }
    pub fn set_address_translation_provider_inner(
        &mut self,
        provider: Arc<dyn PCIeAddressProvider + Sync + Send>,
    ) -> ZxResult {
        if self.is_started(false) {
            return Err(ZxError::BAD_STATE);
        }
        self.address_provider = Some(provider);
        Ok(())
    }
    pub fn add_root_inner(&mut self, root: PciRoot) -> ZxResult {
        if self.is_started(false) {
            return Err(ZxError::BAD_STATE);
        }
        if self.roots.contains_key(&root.managed_bus_id()) {
            return Err(ZxError::ALREADY_EXISTS);
        }
        self.bus_topology.lock();
        self.roots.insert(root.managed_bus_id(), root);
        Ok(())
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

    pub fn start_bus_driver_inner(&mut self) -> ZxResult {
        self.transfer_state(
            PCIeBusDriverState::NotStarted,
            PCIeBusDriverState::StartingScanning,
        )?;
        self.foreach_root(
            |root, _c| {
                root.base_upstream.scan_downstream();
                true
            },
            (),
        );
        self.transfer_state(
            PCIeBusDriverState::StartingScanning,
            PCIeBusDriverState::StartingRunningQuirks,
        )?;
        self.foreach_device(
            &|_root, _c, _level| {
                // PCIeBusDriver::run_quirks(Some(root));
                true
            },
            (),
        );
        // PCIeBusDriver::run_quirks(None);
        self.transfer_state(
            PCIeBusDriverState::StartingRunningQuirks,
            PCIeBusDriverState::StartingResourceAllocation,
        )?;
        self.foreach_root(
            |_root, _c| {
                // root.allocate_downstream_bar();
                true
            },
            (),
        );
        self.transfer_state(
            PCIeBusDriverState::StartingResourceAllocation,
            PCIeBusDriverState::Operational,
        )?;
        Ok(())
    }
    fn foreach_root<T, C>(&self, callback: T, context: C)
    where
        T: Fn(&PciRoot, &mut C) -> bool,
    {
        let mut bus_top_guard = self.bus_topology.lock();
        let mut context = context;
        for (_key, root) in self.roots.iter() {
            drop(bus_top_guard);
            if !callback(root, &mut context) {
                return;
            }
            bus_top_guard = self.bus_topology.lock();
        }
        drop(bus_top_guard);
    }
    fn foreach_device<T, C>(&self, callback: &T, context: C)
    where
        T: Fn(&(dyn IPciNode + Send + Sync), &mut C, usize) -> bool,
    {
        self.foreach_root(
            |root, ctx| {
                self.foreach_downstream(root, 0 /*level*/, callback, &mut (ctx.0));
                true
            },
            (context, &self),
        )
    }
    fn foreach_downstream<T, C>(
        &self,
        upstream: &(dyn IPciNode + Send + Sync),
        level: usize,
        callback: &T,
        context: &mut C,
    ) where
        T: Fn(&(dyn IPciNode + Send + Sync), &mut C, usize) -> bool,
    {
        if level > 256 || upstream.upstream().is_none() {
            return;
        }
        for device in upstream.upstream().unwrap().downstream.iter() {
            if !callback(upstream, context, level) {
                continue;
            }
            if let Some(dev) = device {
                if let PciNodeType::Bridge = dev.node_type() {
                    self.foreach_downstream(dev.as_ref(), level + 1, callback, context);
                }
            }
        }
    }
    fn transfer_state(
        &mut self,
        expected: PCIeBusDriverState,
        target: PCIeBusDriverState,
    ) -> ZxResult {
        if self.state == expected {
            return Err(ZxError::BAD_STATE);
        }
        self.state = target;
        Ok(())
    }
    fn is_started(&self, _allow_quirks_phase: bool) -> bool {
        false
    }

    pub fn get_config(
        &self,
        bus_id: usize,
        dev_id: usize,
        func_id: usize,
    ) -> Option<(Arc<PciConfig>, PhysAddr)> {
        if self.address_provider.is_none() {
            return None;
        }
        let result = self
            .address_provider
            .clone()
            .unwrap()
            .translate(bus_id as u8, dev_id as u8, func_id as u8)
            .ok();
        if result.is_none() {
            return None;
        }
        let (paddr, vaddr) = result.unwrap();
        let mut config = self.configs.lock();
        let cfg = config.iter().find(|x| x.base == vaddr);
        if let Some(x) = cfg {
            return Some((x.clone(), paddr));
        }
        let cfg = self
            .address_provider
            .clone()
            .unwrap()
            .create_config(vaddr as u64);
        config.push(cfg.clone());
        Some((cfg, paddr))
    }

    pub fn link_device_to_upstream(
        &mut self,
        down: Arc<dyn IPciNode + Send + Sync>,
        up: Weak<dyn IPciNode + Send + Sync>,
    ) {
        let _guard = self.bus_topology.lock();
        let dev_down = down.device();
        let dev = dev_down.as_ref().unwrap();
        dev.set_upstream(up.clone());
        let up = up.upgrade().unwrap().upstream().unwrap();
        up.set_downstream(
            dev.dev_id() * PCI_MAX_FUNCTIONS_PER_DEVICE + dev.func_id(),
            Some(down.clone()),
        );
    }

    pub fn find_legacy_irq_handler(&self, irq_id: u32) -> ZxResult<Arc<SharedLegacyIrqHandler>> {
        let mut list = self.legacy_irq_list.lock();
        for i in list.iter() {
            if irq_id == i.irq_id {
                return Ok(i.clone());
            }
        }
        SharedLegacyIrqHandler::create(irq_id)
            .map(|x| {
                list.push(x.clone());
                x
            })
            .ok_or(ZxError::NO_RESOURCES)
    }
}

pub trait PCIeAddressProvider {
    // Creates a config that corresponds to the type of the PcieAddressProvider.
    fn create_config(&self, addr: u64) -> Arc<PciConfig>;
    /// Accepts a PCI BDF triple and returns ZX_OK if it is able to translate it
    /// into an ECAM address.
    fn translate(
        &self,
        bus_id: u8,
        device_id: u8,
        function_id: u8,
    ) -> ZxResult<(PhysAddr, VirtAddr)>;
}

pub struct MmioPcieAddressProvider {
    ecam_regions: Mutex<BTreeMap<u8, MappedEcamRegion>>,
}

impl MmioPcieAddressProvider {
    pub fn new() -> Self {
        MmioPcieAddressProvider {
            ecam_regions: Mutex::new(BTreeMap::new()),
        }
    }
    pub fn add_ecam(&self, ecam: PciEcamRegion) -> ZxResult {
        if ecam.bus_start > ecam.bus_end {
            return Err(ZxError::INVALID_ARGS);
        }
        let bus_count = ecam.bus_end + 1 - ecam.bus_start;
        if ecam.size != bus_count as usize * PCIE_ECAM_BYTES_PER_BUS {
            return Err(ZxError::INVALID_ARGS);
        }
        let mut inner = self.ecam_regions.lock();
        if let Some((_key, value)) = inner.range(..=ecam.bus_start).last() {
            // if intersect...
            if ecam.bus_end <= value.ecam.bus_start
                || value.ecam.bus_end <= ecam.bus_start
                || bus_count == 0
                || value.ecam.bus_start == value.ecam.bus_end + 1
            {
                return Err(ZxError::BAD_STATE);
            }
        }
        let vaddr = kernel_allocate_physical(
            ecam.size,
            ecam.phys_base as PhysAddr,
            MMUFlags::READ | MMUFlags::WRITE,
            CachePolicy::UncachedDevice,
        )?;
        inner.insert(
            ecam.bus_start,
            MappedEcamRegion {
                ecam,
                vaddr: vaddr as u64,
            },
        );
        Ok(())
    }
}

impl PCIeAddressProvider for MmioPcieAddressProvider {
    fn create_config(&self, addr: u64) -> Arc<PciConfig> {
        Arc::new(PciConfig {
            addr_space: PciAddrSpace::MMIO,
            base: addr as usize,
        })
    }
    fn translate(
        &self,
        bus_id: u8,
        device_id: u8,
        function_id: u8,
    ) -> ZxResult<(PhysAddr, VirtAddr)> {
        let regions = self.ecam_regions.lock();
        let target = regions.range(..=bus_id).last().ok_or(ZxError::NOT_FOUND)?;
        if bus_id < target.1.ecam.bus_start || bus_id > target.1.ecam.bus_end {
            return Err(ZxError::NOT_FOUND);
        }
        let bus_id = bus_id - target.1.ecam.bus_start;
        let offset =
            (bus_id as usize) << 20 | (device_id as usize) << 15 | (function_id as usize) << 12;
        let phys = target.1.ecam.phys_base as usize + offset;
        let vaddr = target.1.vaddr as usize + offset;
        Ok((phys, vaddr))
    }
}

pub struct PioPcieAddressProvider {}

impl PioPcieAddressProvider {
    pub fn new() -> Self {
        PioPcieAddressProvider {}
    }
}

impl PCIeAddressProvider for PioPcieAddressProvider {
    fn create_config(&self, addr: u64) -> Arc<PciConfig> {
        Arc::new(PciConfig {
            addr_space: PciAddrSpace::PIO,
            base: addr as usize,
        })
    }
    fn translate(
        &self,
        bus_id: u8,
        device_id: u8,
        function_id: u8,
    ) -> ZxResult<(PhysAddr, VirtAddr)> {
        let virt = pci_bdf_raw_addr(bus_id, device_id, function_id, 0);
        Ok((0, virt as VirtAddr))
    }
}
