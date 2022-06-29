use super::nodes::{
    IPciNode, PciNodeType, PciRoot, PcieBarInfo, PcieIrqMode, PcieIrqModeCaps,
    SharedLegacyIrqHandler,
};
use super::{
    config::PciConfig, constants::*, pci_init_args::PciIrqSwizzleLut, pmio::pci_bdf_raw_addr,
    MappedEcamRegion, PciAddrSpace, PciEcamRegion,
};
use crate::dev::Interrupt;
use crate::object::*;
use crate::vm::{kernel_allocate_physical, CachePolicy, MMUFlags, PhysAddr, VirtAddr};
use crate::ZxResult;

use alloc::sync::{Arc, Weak};
use alloc::{collections::BTreeMap, vec::Vec};
use core::cmp::min;
use core::marker::{Send, Sync};
use lazy_static::*;
use lock::Mutex;
use region_alloc::RegionAllocator;

/// PCIE Bus Driver.
pub struct PCIeBusDriver {
    pub(crate) mmio_lo: Arc<Mutex<RegionAllocator>>,
    pub(crate) mmio_hi: Arc<Mutex<RegionAllocator>>,
    pub(crate) pio_region: Arc<Mutex<RegionAllocator>>,
    address_provider: Option<Arc<dyn PCIeAddressProvider>>,
    roots: BTreeMap<usize, Arc<PciRoot>>,
    state: PCIeBusDriverState,
    bus_topology: Mutex<()>,
    configs: Mutex<Vec<Arc<PciConfig>>>,
    legacy_irq_list: Mutex<Vec<Arc<SharedLegacyIrqHandler>>>,
}

#[derive(PartialEq, Debug)]
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
    /// Add bus region.
    pub fn add_bus_region(base: u64, size: u64, aspace: PciAddrSpace) -> ZxResult {
        _INSTANCE.lock().add_bus_region_inner(base, size, aspace)
    }
    /// Subtract bus region.
    pub fn sub_bus_region(base: u64, size: u64, aspace: PciAddrSpace) -> ZxResult {
        _INSTANCE.lock().sub_bus_region_inner(base, size, aspace)
    }
    /// A PcieAddressProvider translates a BDF address to an address that the
    /// system can use to access ECAMs.
    pub fn set_address_translation_provider(provider: Arc<dyn PCIeAddressProvider>) -> ZxResult {
        _INSTANCE
            .lock()
            .set_address_translation_provider_inner(provider)
    }

    /// Add a root bus to the driver and attempt to scan it for devices.
    pub fn add_root(bus_id: usize, lut: PciIrqSwizzleLut) -> ZxResult {
        let mut bus = _INSTANCE.lock();
        let root = PciRoot::new(bus_id, lut, &bus);
        bus.add_root_inner(root)
    }

    /// Start the bus driver.
    pub fn start_bus_driver() -> ZxResult {
        _INSTANCE.lock().start_bus_driver_inner()
    }

    /// Get the "Nth" device.
    pub fn get_nth_device(n: usize) -> ZxResult<(PcieDeviceInfo, Arc<PcieDeviceKObject>)> {
        let device_node = _INSTANCE
            .lock()
            .get_nth_device_inner(n)
            .ok_or(ZxError::OUT_OF_RANGE)?;
        let device = device_node.device();
        let info = PcieDeviceInfo {
            vendor_id: device.vendor_id,
            device_id: device.device_id,
            base_class: device.class_id,
            sub_class: device.subclass_id,
            program_interface: device.prog_if,
            revision_id: device.rev_id,
            bus_id: device.bus_id as u8,
            dev_id: device.dev_id as u8,
            func_id: device.func_id as u8,
            _padding1: 0,
        };
        let object = PcieDeviceKObject::new(device_node.clone());
        Ok((info, object))
    }
}

impl PCIeBusDriver {
    fn new() -> Self {
        PCIeBusDriver {
            mmio_lo: Default::default(),
            mmio_hi: Default::default(),
            pio_region: Default::default(),
            address_provider: None,
            roots: BTreeMap::new(),
            state: PCIeBusDriverState::NotStarted,
            bus_topology: Mutex::default(),
            legacy_irq_list: Mutex::new(Vec::new()),
            configs: Mutex::new(Vec::new()),
        }
    }
    fn add_bus_region_inner(&mut self, base: u64, size: u64, aspace: PciAddrSpace) -> ZxResult {
        self.add_or_sub_bus_region(base, size, aspace, true)
    }
    fn sub_bus_region_inner(&mut self, base: u64, size: u64, aspace: PciAddrSpace) -> ZxResult {
        self.add_or_sub_bus_region(base, size, aspace, false)
    }
    fn set_address_translation_provider_inner(
        &mut self,
        provider: Arc<dyn PCIeAddressProvider>,
    ) -> ZxResult {
        if self.is_started(false) {
            return Err(ZxError::BAD_STATE);
        }
        self.address_provider = Some(provider);
        Ok(())
    }
    fn add_root_inner(&mut self, root: Arc<PciRoot>) -> ZxResult {
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
                self.mmio_lo
                    .lock()
                    .add_or_subtract(base as usize, lo_size as usize, is_add);
            }
            if end > u32_max + 1 {
                let hi_size = min(end - (u32_max + 1), size);
                self.mmio_hi
                    .lock()
                    .add_or_subtract((end - hi_size) as usize, end as usize, is_add);
            }
        } else if aspace == PciAddrSpace::PIO {
            let end = base + size - 1;
            if ((base | end) & !PCIE_PIO_ADDR_SPACE_MASK) != 0 {
                return Err(ZxError::INVALID_ARGS);
            }
            self.pio_region
                .lock()
                .add_or_subtract(base as usize, size as usize, is_add);
        }
        Ok(())
    }

    fn start_bus_driver_inner(&mut self) -> ZxResult {
        self.transfer_state(
            PCIeBusDriverState::NotStarted,
            PCIeBusDriverState::StartingScanning,
        )?;
        self.foreach_root(
            |root, _c| {
                root.base_upstream.scan_downstream(self);
                true
            },
            (),
        );
        self.transfer_state(
            PCIeBusDriverState::StartingScanning,
            PCIeBusDriverState::StartingRunningQuirks,
        )?;
        warn!("pci: skip quirks");
        self.transfer_state(
            PCIeBusDriverState::StartingRunningQuirks,
            PCIeBusDriverState::StartingResourceAllocation,
        )?;
        self.foreach_root(
            |root, _| {
                root.base_upstream.allocate_downstream_bars();
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
    fn foreach_root<T, C>(&self, callback: T, context: C) -> C
    where
        T: Fn(Arc<PciRoot>, &mut C) -> bool,
    {
        let mut bus_top_guard = self.bus_topology.lock();
        let mut context = context;
        for (_key, root) in self.roots.iter() {
            drop(bus_top_guard);
            if !callback(root.clone(), &mut context) {
                return context;
            }
            bus_top_guard = self.bus_topology.lock();
        }
        drop(bus_top_guard);
        context
    }

    #[allow(dead_code)]
    fn foreach_device<T, C>(&self, callback: &T, context: C) -> C
    where
        T: Fn(Arc<dyn IPciNode>, &mut C, usize) -> bool,
    {
        self.foreach_root(
            |root, ctx| {
                self.foreach_downstream(root, 0 /*level*/, callback, &mut (ctx.0))
            },
            (context, &self),
        )
        .0
    }

    #[allow(dead_code)]
    fn foreach_downstream<T, C>(
        &self,
        upstream: Arc<dyn IPciNode>,
        level: usize,
        callback: &T,
        context: &mut C,
    ) -> bool
    where
        T: Fn(Arc<dyn IPciNode>, &mut C, usize) -> bool,
    {
        if level > 256 || upstream.as_upstream().is_none() {
            return true;
        }
        let upstream = upstream.as_upstream().unwrap();
        for i in 0..PCI_MAX_FUNCTIONS_PER_BUS {
            let device = upstream.get_downstream(i);
            if let Some(dev) = device {
                if !callback(dev.clone(), context, level) {
                    return false;
                }
                if let PciNodeType::Bridge = dev.node_type() {
                    if !self.foreach_downstream(dev, level + 1, callback, context) {
                        return false;
                    }
                }
            }
        }
        true
    }
    fn transfer_state(
        &mut self,
        expected: PCIeBusDriverState,
        target: PCIeBusDriverState,
    ) -> ZxResult {
        trace!("transfer state from {:#x?} to {:#x?}", expected, target);
        if self.state != expected {
            return Err(ZxError::BAD_STATE);
        }
        self.state = target;
        Ok(())
    }
    fn is_started(&self, _allow_quirks_phase: bool) -> bool {
        !matches!(self.state, PCIeBusDriverState::NotStarted)
    }

    /// Get a device's config.
    pub fn get_config(
        &self,
        bus_id: usize,
        dev_id: usize,
        func_id: usize,
    ) -> Option<(Arc<PciConfig>, PhysAddr)> {
        self.address_provider.as_ref()?;
        let (paddr, vaddr) = self
            .address_provider
            .clone()
            .unwrap()
            .translate(bus_id as u8, dev_id as u8, func_id as u8)
            .ok()?;
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

    /// Link a device to an upstream node.
    pub fn link_device_to_upstream(&self, down: Arc<dyn IPciNode>, up: Weak<dyn IPciNode>) {
        let _guard = self.bus_topology.lock();
        let dev = down.device();
        dev.set_upstream(up.clone());
        let up = up.upgrade().unwrap().as_upstream().unwrap();
        up.set_downstream(
            dev.dev_id() * PCI_MAX_FUNCTIONS_PER_DEVICE + dev.func_id(),
            Some(down.clone()),
        );
    }

    /// Find the legacy IRQ handler.
    pub fn find_legacy_irq_handler(&self, irq_id: usize) -> ZxResult<Arc<SharedLegacyIrqHandler>> {
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

    fn get_nth_device_inner(&self, n: usize) -> Option<Arc<dyn IPciNode>> {
        self.foreach_device(
            &|device, context: &mut (usize, Option<Arc<_>>), _level| {
                if context.0 == 0 {
                    context.1 = Some(device);
                    false
                } else {
                    context.0 -= 1;
                    true
                }
            },
            (n, None),
        )
        .1
    }
}

/// PcieAddressProvider is an interface that implements translation from a BDF to
/// a PCI ECAM address.
pub trait PCIeAddressProvider: Send + Sync {
    /// Creates a config that corresponds to the type of the PcieAddressProvider.
    fn create_config(&self, addr: u64) -> Arc<PciConfig>;

    /// Accepts a PCI BDF triple and returns ZX_OK if it is able to translate it
    /// into an ECAM address.
    fn translate(&self, bus_id: u8, dev_id: u8, func_id: u8) -> ZxResult<(PhysAddr, VirtAddr)>;
}

/// Systems that have memory mapped Config Spaces.
#[derive(Default)]
pub struct MmioPcieAddressProvider {
    ecam_regions: Mutex<BTreeMap<u8, MappedEcamRegion>>,
}

impl MmioPcieAddressProvider {
    /// Add a ECAM region.
    pub fn add_ecam(&self, ecam: PciEcamRegion) -> ZxResult {
        if ecam.bus_start > ecam.bus_end {
            return Err(ZxError::INVALID_ARGS);
        }
        let bus_count = (ecam.bus_end - ecam.bus_start) as usize + 1;
        if ecam.size != bus_count * PCIE_ECAM_BYTES_PER_BUS {
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

/// Systems that have PIO mapped Config Spaces.
#[derive(Default)]
pub struct PmioPcieAddressProvider;

impl PCIeAddressProvider for PmioPcieAddressProvider {
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

/// Info returned to dev manager for PCIe devices when probing.
#[allow(missing_docs)]
#[repr(C)]
#[derive(Clone, Debug)]
pub struct PcieDeviceInfo {
    pub vendor_id: u16,
    pub device_id: u16,
    pub base_class: u8,
    pub sub_class: u8,
    pub program_interface: u8,
    pub revision_id: u8,
    pub bus_id: u8,
    pub dev_id: u8,
    pub func_id: u8,
    _padding1: u8,
}

/// PCIE Device Entity.
pub struct PcieDeviceKObject {
    base: KObjectBase,
    device: Arc<dyn IPciNode>,
    irqs_avail_cnt: u32, // WARNING
    irqs_maskable: bool, // WARNING
}

impl_kobject!(PcieDeviceKObject);

impl PcieDeviceKObject {
    /// Create a new PcieDeviceKObject.
    pub fn new(device: Arc<dyn IPciNode>) -> Arc<PcieDeviceKObject> {
        Arc::new(PcieDeviceKObject {
            base: KObjectBase::new(),
            device,
            irqs_avail_cnt: 10,  // WARNING
            irqs_maskable: true, // WARNING
        })
    }

    /// Get PcieBarInfo.
    pub fn get_bar(&self, bar_num: u32) -> ZxResult<PcieBarInfo> {
        let device = self.device.device();
        device.get_bar(bar_num as usize).ok_or(ZxError::NOT_FOUND)
    }

    /// Map the interrupt to the IRQ.
    pub fn map_interrupt(&self, irq: i32) -> ZxResult<Arc<Interrupt>> {
        if irq < 0 || irq as u32 >= self.irqs_avail_cnt {
            return Err(ZxError::INVALID_ARGS);
        }
        Interrupt::new_pci(self.device.clone(), irq as u32, self.irqs_maskable)
    }

    /// Enable MMIO.
    pub fn enable_mmio(&self) -> ZxResult {
        self.device.device().enable_mmio(true)
    }

    /// Enable PIO.
    pub fn enable_pio(&self) -> ZxResult {
        self.device.device().enable_pio(true)
    }

    /// Enable bus mastering.
    pub fn enable_master(&self, enable: bool) -> ZxResult {
        self.device.device().enable_master(enable)
    }

    /// Check whether `mode` is capable PCI device's IRQ modes.
    pub fn get_irq_mode_capabilities(&self, mode: PcieIrqMode) -> ZxResult<PcieIrqModeCaps> {
        self.device.device().get_irq_mode_capabilities(mode)
    }

    /// Set IRQ mode.
    pub fn set_irq_mode(&self, mode: PcieIrqMode, requested_irqs: usize) -> ZxResult {
        self.device.device().set_irq_mode(mode, requested_irqs)
    }

    /// Read the device's config.
    pub fn config_read(&self, offset: usize, width: usize) -> ZxResult<u32> {
        self.device.device().config_read(offset, width)
    }

    /// Write the device's config.
    pub fn config_write(&self, offset: usize, width: usize, val: u32) -> ZxResult {
        self.device.device().config_write(offset, width, val)
    }
}
