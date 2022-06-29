#![allow(dead_code)]
#![allow(missing_docs)]

use super::caps::{
    PciCapAdvFeatures, PciCapPcie, PciCapability, PciCapabilityMsi, PciCapabilityStd, PciMsiBlock,
};
use super::config::{
    PciConfig, PciReg16, PciReg32, PciReg8, PCIE_BASE_CONFIG_SIZE, PCIE_EXTENDED_CONFIG_SIZE,
};
use super::constants::*;
use super::{bus::PCIeBusDriver, pci_init_args::PciIrqSwizzleLut};
use crate::{vm::PAGE_SIZE, ZxError, ZxResult};

use alloc::sync::{Arc, Weak};
use alloc::{boxed::Box, vec::Vec};
use kernel_hal::interrupt;
use lock::{Mutex, MutexGuard};
use numeric_enum_macro::numeric_enum;
use region_alloc::RegionAllocator;

numeric_enum! {
    #[repr(u8)]
    #[derive(PartialEq, Copy, Clone, Debug)]
    pub enum PcieDeviceType {
        Unknown = 0xFF,
        PcieEndpoint = 0x0,
        LegacyPcieEndpoint = 0x1,
        RcIntegratedEndpoint = 0x9,
        RcEventCollector = 0xA,
        // Type 1 config header types
        RcRootPort = 0x4,
        SwitchUpstreamPort = 0x5,
        SwitchDownstreamPort = 0x6,
        PcieToPciBridge = 0x7,
        PciToPcieBridge = 0x8,
    }
}

pub struct PcieUpstream {
    managed_bus_id: usize,
    inner: Mutex<PcieUpstreamInner>,
}

struct PcieUpstreamInner {
    weak_super: Weak<dyn IPciNode>,
    downstream: Box<[Option<Arc<dyn IPciNode>>]>,
}

impl PcieUpstream {
    pub fn create(managed_bus_id: usize) -> Arc<Self> {
        Arc::new(PcieUpstream {
            managed_bus_id,
            inner: Mutex::new(PcieUpstreamInner {
                weak_super: Weak::<PciRoot>::new(),
                downstream: {
                    let mut vec =
                        Vec::<Option<Arc<dyn IPciNode>>>::with_capacity(PCI_MAX_FUNCTIONS_PER_BUS);
                    vec.resize(PCI_MAX_FUNCTIONS_PER_BUS, None);
                    vec.into_boxed_slice()
                },
            }),
        })
    }

    pub fn scan_downstream(&self, driver: &PCIeBusDriver) {
        for dev_id in 0..PCI_MAX_DEVICES_PER_BUS {
            for func_id in 0..PCI_MAX_FUNCTIONS_PER_DEVICE {
                let cfg = driver.get_config(self.managed_bus_id, dev_id, func_id);
                if cfg.is_none() {
                    warn!("bus being scanned is outside ecam region!");
                    return;
                }
                let (cfg, _paddr) = cfg.unwrap();
                let vendor_id = cfg.read16(PciReg16::VendorId);
                let mut good_device = vendor_id as usize != PCIE_INVALID_VENDOR_ID;
                if good_device {
                    let device_id = cfg.read16(PciReg16::DeviceId);
                    info!(
                        "Found device {:#x?}:{:#x?} at {:#x?}:{:#x?}.{:#x?}",
                        vendor_id, device_id, self.managed_bus_id, dev_id, func_id
                    );
                    let ndx = dev_id * PCI_MAX_FUNCTIONS_PER_DEVICE + func_id;
                    let downstream_device = self.get_downstream(ndx);
                    match downstream_device {
                        Some(dev) => {
                            if let PciNodeType::Bridge = dev.node_type() {
                                dev.as_upstream().unwrap().scan_downstream(driver);
                            }
                        }
                        None => {
                            if self
                                .scan_device(cfg.as_ref(), dev_id, func_id, Some(vendor_id), driver)
                                .is_none()
                            {
                                info!(
                                    "failed to initialize device {:#x?}:{:#x?}.{:#x?}",
                                    self.managed_bus_id, dev_id, func_id
                                );
                                good_device = false;
                            }
                        }
                    }
                    info!("a device is discovered");
                }
                // At the point of function #0, if either there is no device, or cfg's
                // header indicates that it is not a multi-function device, just move on to
                // next device
                if func_id == 0
                    && (!good_device
                        || (cfg.read8(PciReg8::HeaderType) & PCI_HEADER_TYPE_MULTI_FN) != 0)
                {
                    break;
                }
            }
        }
    }

    pub fn allocate_downstream_bars(&self) {
        for dev_id in 0..PCI_MAX_DEVICES_PER_BUS {
            let dev = self.get_downstream(dev_id);
            if dev.is_none() {
                continue;
            }
            let dev = dev.unwrap();
            if dev.allocate_bars().is_err() {
                error!("Allocate Bar Fail");
                dev.disable();
            }
        }
    }

    fn scan_device(
        &self,
        cfg: &PciConfig,
        dev_id: usize,
        func_id: usize,
        vendor_id: Option<u16>,
        driver: &PCIeBusDriver,
    ) -> Option<Arc<dyn IPciNode>> {
        let vendor_id = vendor_id.unwrap_or_else(|| cfg.read16(PciReg16::VendorId));
        if vendor_id == PCIE_INVALID_VENDOR_ID as u16 {
            return None;
        }
        let header_type = cfg.read8(PciReg8::HeaderType) & 0x7f;
        let weak_super = self.inner.lock().weak_super.clone();
        if header_type == PCI_HEADER_TYPE_PCI_BRIDGE {
            let secondary_id = cfg.read8(PciReg8::SecondaryBusId);
            PciBridge::new(weak_super, dev_id, func_id, secondary_id as usize, driver)
                .map(|x| x as _)
        } else {
            PciDeviceNode::new(weak_super, dev_id, func_id, driver).map(|x| x as _)
        }
    }

    pub fn get_downstream(&self, index: usize) -> Option<Arc<dyn IPciNode>> {
        if index >= PCI_MAX_FUNCTIONS_PER_BUS {
            return None;
        }
        self.inner.lock().downstream[index].clone()
    }
    pub fn set_downstream(&self, index: usize, down: Option<Arc<dyn IPciNode>>) {
        self.inner.lock().downstream[index] = down;
    }

    pub fn set_super(&self, weak_super: Weak<dyn IPciNode>) {
        self.inner.lock().weak_super = weak_super;
    }
}

/// Struct used to fetch information about a configured base address register.
#[allow(missing_docs)]
#[derive(Default, Debug, Copy, Clone)]
pub struct PcieBarInfo {
    pub is_mmio: bool,
    pub is_64bit: bool,
    pub is_prefetchable: bool,
    pub first_bar_reg: usize,
    pub size: u64,
    pub bus_addr: u64,
    allocation: Option<(usize, usize)>,
}

/// Struct for managing shared legacy IRQ handlers.
#[derive(Default)]
pub struct SharedLegacyIrqHandler {
    /// The IRQ id.
    pub irq_id: usize,
    device_handler: Mutex<Vec<Arc<PcieDevice>>>,
}

impl SharedLegacyIrqHandler {
    /// Create a new SharedLegacyIrqHandler.
    pub fn create(irq_id: usize) -> Option<Arc<SharedLegacyIrqHandler>> {
        info!("SharedLegacyIrqHandler created for {:#x?}", irq_id);
        interrupt::mask_irq(irq_id).unwrap();
        let handler = Arc::new(SharedLegacyIrqHandler {
            irq_id,
            device_handler: Mutex::new(Vec::new()),
        });
        let handler_copy = handler.clone();
        interrupt::register_irq_handler(irq_id, Box::new(move || handler_copy.handle())).ok()?;
        Some(handler)
    }

    /// Handle the IRQ.
    pub fn handle(&self) {
        let device_handler = self.device_handler.lock();
        if device_handler.is_empty() {
            interrupt::mask_irq(self.irq_id).unwrap();
            return;
        }
        for dev in device_handler.iter() {
            let cfg = dev.config().unwrap();
            let _command = cfg.read16(PciReg16::Command);
            // let status = cfg.read16(PciReg16::Status);
            // if (command & PCIE_CFG_COMMAND_INT_DISABLE) != 0 {
            //     continue;
            // }
            let inner = dev.inner.lock();
            let handler_lock = inner.irq.handlers[0].handler.lock();
            let handler = if inner.irq.handlers.is_empty() {
                None
            } else {
                let handler = &inner.irq.handlers[0];
                if handler.get_masked() {
                    handler_lock.as_ref()
                } else {
                    None
                }
            };
            let ret = if let Some(h) = handler {
                let code = h();
                if (code & PCIE_IRQRET_MASK) != 0 {
                    inner.irq.handlers[0].set_masked(true);
                }
                code
            } else {
                PCIE_IRQRET_MASK
            };
            if (ret & PCIE_IRQRET_MASK) != 0 {
                cfg.write16(
                    PciReg16::Command,
                    cfg.read16(PciReg16::Command) | PCIE_CFG_COMMAND_INT_DISABLE,
                );
            }
        }
    }
    pub fn add_device(&self, device: Arc<PcieDevice>) {
        let cfg = device.config().unwrap();
        cfg.write16(
            PciReg16::Command,
            cfg.read16(PciReg16::Command) | PCIE_CFG_COMMAND_INT_DISABLE,
        );
        let mut device_handler = self.device_handler.lock();
        let is_first = device_handler.is_empty();
        device_handler.push(device);
        if is_first {
            interrupt::unmask_irq(self.irq_id).unwrap();
        }
    }
    pub fn remove_device(&self, device: Arc<PcieDevice>) {
        let cfg = device.config().unwrap();
        cfg.write16(
            PciReg16::Command,
            cfg.read16(PciReg16::Command) | PCIE_CFG_COMMAND_INT_DISABLE,
        );
        let mut device_handler = self.device_handler.lock();
        device_handler.retain(|h| Arc::ptr_eq(h, &device));
        if device_handler.is_empty() {
            interrupt::mask_irq(self.irq_id).unwrap();
        }
    }
}

numeric_enum! {
    #[repr(u32)]
    #[derive(Debug, PartialEq, Eq, Copy, Clone)]
      /// Enumeration which defines the IRQ modes a PCIe device may be operating in.
      pub enum PcieIrqMode {
        /// All IRQs are disabled.  0 total IRQs are supported in this mode.
        Disabled = 0,
        ///    Devices may support up to 1 legacy IRQ in total.  Exclusive IRQ access
        ///    cannot be guaranteed (the IRQ may be shared with other devices)
        Legacy = 1,
        /// Devices may support up to 32 MSI IRQs in total.  IRQs may be allocated
        ///    exclusively, resources permitting.
        Msi = 2,
        ///   Devices may support up to 2048 MSI-X IRQs in total.  IRQs may be allocated
        ///   exclusively, resources permitting.
        MsiX = 3,
        #[allow(missing_docs)]
        Count = 4,
    }
}

impl Default for PcieIrqMode {
    fn default() -> Self {
        PcieIrqMode::Disabled
    }
}

/// Struct for managing IRQ handlers.
pub struct PcieIrqHandle {
    handle: Option<Box<dyn Fn() + Send + Sync>>,
    enabled: bool,
}

#[derive(Default)]
pub struct PcieLegacyIrqState {
    pub pin: u8,
    pub id: usize,
    pub shared_handler: Arc<SharedLegacyIrqHandler>,
    pub handlers: Vec<PcieIrqHandle>, // WARNING
    pub mode: PcieIrqMode,            // WANRING
    pub msi: Option<PciCapabilityMsi>,
    pub pcie: Option<PciCapPcie>,
}

pub struct PcieIrqState {
    pub legacy: PcieLegacyIrqState,
    pub mode: PcieIrqMode,
    pub handlers: Vec<Arc<PcieIrqHandlerState>>,
    pub registered_handler_count: usize,
}

impl Default for PcieIrqState {
    fn default() -> Self {
        Self {
            legacy: Default::default(),
            mode: PcieIrqMode::Disabled,
            handlers: Vec::default(),
            registered_handler_count: 0,
        }
    }
}

/// Class for managing shared legacy IRQ handlers.
#[derive(Default)]
pub struct PcieIrqHandlerState {
    irq_id: usize,
    masked: Mutex<bool>,
    enabled: Mutex<bool>,
    handler: Mutex<Option<Box<dyn Fn() -> u32 + Send + Sync>>>,
}

impl PcieIrqHandlerState {
    pub fn set_masked(&self, masked: bool) {
        *self.masked.lock() = masked;
    }
    pub fn get_masked(&self) -> bool {
        *self.masked.lock()
    }
    pub fn set_handler(&self, h: Option<Box<dyn Fn() -> u32 + Send + Sync>>) {
        *self.handler.lock() = h;
    }
    pub fn has_handler(&self) -> bool {
        self.handler.lock().is_some()
    }
    pub fn enable(&self, e: bool) {
        *self.enabled.lock() = e;
    }
}

pub struct PcieDevice {
    pub bus_id: usize,
    pub dev_id: usize,
    pub func_id: usize,
    // pub is_bridge: bool,
    pub bar_count: usize,
    cfg: Option<Arc<PciConfig>>,
    cfg_phys: usize,
    dev_lock: Mutex<()>,
    command_lock: Mutex<()>,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class_id: u8,
    pub subclass_id: u8,
    pub prog_if: u8,
    pub rev_id: u8,
    pub inner: Mutex<PcieDeviceInner>,
}

pub struct PcieDeviceInner {
    pub irq: PcieIrqState,
    pub bars: [PcieBarInfo; 6],
    pub caps: Vec<PciCapability>,
    pub plugged_in: bool,
    pub upstream: Weak<(dyn IPciNode)>,
    pub weak_super: Weak<(dyn IPciNode)>,
    pub disabled: bool,
}

impl Default for PcieDeviceInner {
    fn default() -> Self {
        PcieDeviceInner {
            irq: Default::default(),
            bars: Default::default(),
            caps: Default::default(),
            plugged_in: false,
            upstream: Weak::<PciRoot>::new(),
            weak_super: Weak::<PciRoot>::new(),
            disabled: false,
        }
    }
}

impl PcieDeviceInner {
    pub fn arc_self(&self) -> Arc<PcieDevice> {
        self.weak_super.upgrade().unwrap().device()
    }
    pub fn msi(&self) -> Option<(&PciCapabilityStd, &PciCapabilityMsi)> {
        for c in self.caps.iter() {
            if let PciCapability::Msi(std, msi) = c {
                if std.is_valid() {
                    return Some((std, msi));
                }
            }
        }
        None
    }
    pub fn pcie(&self) -> Option<(&PciCapabilityStd, &PciCapPcie)> {
        for c in self.caps.iter() {
            if let PciCapability::Pcie(std, pcie) = c {
                if std.is_valid() {
                    return Some((std, pcie));
                }
            }
        }
        None
    }
}

impl PcieDevice {
    pub fn create(
        upstream: Weak<dyn IPciNode>,
        dev_id: usize,
        func_id: usize,
        driver: &PCIeBusDriver,
    ) -> Option<Arc<Self>> {
        let ups = upstream.upgrade().unwrap().as_upstream()?;
        let (cfg, paddr) = driver.get_config(ups.managed_bus_id, dev_id, func_id)?;
        let inst = Arc::new(PcieDevice {
            bus_id: ups.managed_bus_id,
            dev_id,
            func_id,
            // is_bridge: false,
            bar_count: 6, // PCIE BAR regs per device
            cfg: Some(cfg.clone()),
            cfg_phys: paddr,
            dev_lock: Mutex::default(),
            command_lock: Mutex::default(),
            vendor_id: cfg.read16(PciReg16::VendorId),
            device_id: cfg.read16(PciReg16::DeviceId),
            class_id: cfg.read8(PciReg8::BaseClass),
            subclass_id: cfg.read8(PciReg8::SubClass),
            prog_if: cfg.read8(PciReg8::ProgramInterface),
            rev_id: cfg.read8(PciReg8::RevisionId),
            inner: Default::default(),
        });
        inst.init(upstream, driver).unwrap();
        Some(inst)
    }
    fn init(&self, upstream: Weak<dyn IPciNode>, driver: &PCIeBusDriver) -> ZxResult {
        info!("init PciDevice");
        self.init_probe_bars()?;
        self.init_capabilities()?;
        self.init_legacy_irq(&upstream, driver)?;
        let mut inner = self.inner.lock();
        inner.plugged_in = true;
        // let sup = inner.weak_super.upgrade().unwrap().clone();
        drop(inner);
        // driver.link_device_to_upstream(sup, upstream);
        Ok(())
    }

    fn init_probe_bars(&self) -> ZxResult {
        info!("init PciDevice probe bars");
        // probe bars
        let mut i = 0;
        let cfg = self.cfg.as_ref().unwrap();
        while i < self.bar_count {
            let bar_val = cfg.read_bar(i);
            let is_mmio = (bar_val & PCI_BAR_IO_TYPE_MASK) == PCI_BAR_IO_TYPE_MMIO;
            let is_64bit = is_mmio && (bar_val & PCI_BAR_MMIO_TYPE_MASK) == PCI_BAR_MMIO_TYPE_64BIT;
            if is_64bit {
                if i + 1 >= self.bar_count {
                    warn!(
                        "Illegal 64-bit MMIO BAR position {}/{} while fetching BAR info",
                        i, self.bar_count
                    );
                    return Err(ZxError::BAD_STATE);
                }
            } else if is_mmio && ((bar_val & PCI_BAR_MMIO_TYPE_MASK) != PCI_BAR_MMIO_TYPE_32BIT) {
                warn!(
                    "Unrecognized MMIO BAR type (BAR[{}] == {:#x?}) while fetching BAR info",
                    i, bar_val
                );
                return Err(ZxError::BAD_STATE);
            }
            // Disable either MMIO or PIO (depending on the BAR type) access while we perform the probe.
            // let _cmd_lock = self.command_lock.lock(); lock is useless during init
            let backup = cfg.read16(PciReg16::Command);
            cfg.write16(
                PciReg16::Command,
                backup
                    & !(if is_mmio {
                        PCI_COMMAND_MEM_EN
                    } else {
                        PCI_COMMAND_IO_EN
                    }),
            );
            // Figure out the size of this BAR region by writing 1's to the address bits
            let addr_mask = if is_mmio {
                PCI_BAR_MMIO_ADDR_MASK
            } else {
                PCI_BAR_PIO_ADDR_MASK
            };
            let addr_lo = bar_val & addr_mask;
            cfg.write_bar(i, bar_val | addr_mask);
            let mut size_mask: u64 = !(cfg.read_bar(i) & addr_mask) as u64;
            cfg.write_bar(i, bar_val);
            if is_mmio && is_64bit {
                let bar_id = i + 1;
                let bar_val = cfg.read_bar(bar_id);
                cfg.write_bar(bar_id, 0xFFFF_FFFF);
                size_mask |= (!cfg.read_bar(bar_id) as u64) << 32;
                cfg.write_bar(bar_id, bar_val);
            }
            cfg.write16(PciReg16::Command, backup);
            let size = if is_64bit {
                size_mask + 1
            } else {
                (size_mask + 1) as u32 as u64
            };
            let size = if is_mmio {
                size
            } else {
                size & PCIE_PIO_ADDR_SPACE_MASK
            };
            let bus_addr = if is_mmio && is_64bit {
                (addr_lo as u64) | ((bar_val as u64) << 32)
            } else {
                addr_lo as u64
            };
            let bar_info = PcieBarInfo {
                is_mmio,
                is_64bit,
                is_prefetchable: is_mmio && (bar_val & PCI_BAR_MMIO_PREFETCH_MASK) != 0,
                first_bar_reg: i,
                size,
                bus_addr,
                allocation: None,
            };
            let bar_info_size = bar_info.size;
            self.inner.lock().bars[i] = bar_info;
            i += 1;
            if is_64bit && bar_info_size > 0 {
                i += 1;
                if i > self.bar_count {
                    return Err(ZxError::BAD_STATE);
                }
            }
        }
        Ok(())
    }
    fn init_capabilities(&self) -> ZxResult {
        info!("init PciDevice caps");
        let cfg = self.cfg.as_ref().unwrap();
        let mut cap_offset = cfg.read8(PciReg8::CapabilitiesPtr);
        let mut found_num = 0;
        while cap_offset != 0 && found_num < (256 - 64) / 4 {
            if cap_offset == 0xff || cap_offset < 64 || cap_offset > 252 {
                return Err(ZxError::INVALID_ARGS);
            }
            let id = cfg.read8_(cap_offset as usize);
            let std = PciCapabilityStd::create(cap_offset as u16, id);
            let mut inner = self.inner.lock();
            let cap = match id {
                0x5 => PciCapability::Msi(
                    std,
                    PciCapabilityMsi::create(cfg.as_ref(), cap_offset as usize, id),
                ),
                0x10 => PciCapability::Pcie(
                    std,
                    PciCapPcie::create(cfg.as_ref(), cap_offset as u16, id),
                ),
                0x13 => PciCapability::AdvFeatures(
                    std,
                    PciCapAdvFeatures::create(cfg.as_ref(), cap_offset as u16, id),
                ),
                _ => PciCapability::Std(std),
            };
            inner.caps.push(cap);
            cap_offset = cfg.read8_(cap_offset as usize + 1) & 0xFC;
            found_num += 1;
        }
        Ok(())
    }
    fn init_legacy_irq(&self, upstream: &Weak<dyn IPciNode>, driver: &PCIeBusDriver) -> ZxResult {
        info!("init PciDevice legacy irq");
        self.modify_cmd(0, 1 << 10);
        let cfg = self.cfg.as_ref().unwrap();
        let pin = cfg.read8(PciReg8::InterruptPin);
        let mut inner = self.inner.lock();
        inner.irq.legacy.pin = pin;
        if pin != 0 {
            inner.irq.legacy.id = self.map_pin_to_irq_locked(upstream, pin)?;
            inner.irq.legacy.shared_handler =
                driver.find_legacy_irq_handler(inner.irq.legacy.id)?;
        }
        Ok(())
    }
    fn map_pin_to_irq_locked(
        &self,
        // _lock: &MutexGuard<()>, lock is useless during init
        upstream: &Weak<dyn IPciNode>,
        mut pin: u8,
    ) -> ZxResult<usize> {
        // Don't use self.inner.lock() in this function !!!
        if pin == 0 || pin > 4 {
            return Err(ZxError::BAD_STATE);
        }
        pin -= 1;
        let mut dev_id = self.dev_id;
        let mut func_id = self.func_id;
        let mut upstream = upstream.clone();
        while let Some(up) = upstream.upgrade() {
            if let PciNodeType::Bridge = up.node_type() {
                let bdev = up.device();
                match bdev.pcie_device_type() {
                    PcieDeviceType::Unknown
                    | PcieDeviceType::SwitchUpstreamPort
                    | PcieDeviceType::PcieToPciBridge
                    | PcieDeviceType::PciToPcieBridge => {
                        pin = (pin + dev_id as u8) % 4;
                    }
                    _ => (),
                }
                let dev = up.device();
                dev_id = dev.dev_id;
                func_id = dev.func_id;
                upstream = dev.upstream();
            } else {
                break;
            }
        }
        let upstream = upstream.upgrade();
        if let Some(up_ptr) = upstream {
            if let Some(up) = up_ptr.as_root() {
                return up.swizzle(dev_id, func_id, pin as usize);
            }
        }
        Err(ZxError::BAD_STATE)
    }

    pub fn allocate_bars(&self) -> ZxResult {
        let mut inner = self.inner.lock();
        assert!(inner.plugged_in);
        for i in 0..self.bar_count {
            let bar_info = &inner.bars[i];
            if bar_info.size == 0 || bar_info.allocation.is_some() {
                continue;
            }
            let upstream = inner.upstream.upgrade().ok_or(ZxError::UNAVAILABLE)?;
            let mut bar_info = &mut inner.bars[i];
            if bar_info.bus_addr != 0 {
                let allocator =
                    if upstream.node_type() == PciNodeType::Bridge && bar_info.is_prefetchable {
                        Some(upstream.pf_mmio_regions())
                    } else if bar_info.is_mmio {
                        let inclusive_end = bar_info.bus_addr + bar_info.size - 1;
                        if inclusive_end <= u32::MAX.into() {
                            Some(upstream.mmio_lo_regions())
                        } else if bar_info.bus_addr > u32::MAX.into() {
                            Some(upstream.mmio_hi_regions())
                        } else {
                            None
                        }
                    } else {
                        Some(upstream.pio_regions())
                    };
                if let Some(allocator) = allocator {
                    let base: usize = bar_info.bus_addr as _;
                    let size: usize = bar_info.size as _;
                    if allocator.lock().allocate_by_addr(base, size) {
                        bar_info.allocation = Some((base, size));
                        continue;
                    }
                }
                error!("Failed to preserve device window");
                bar_info.bus_addr = 0;
            }
            warn!("No bar addr for {}...", i);
            self.assign_cmd(PCIE_CFG_COMMAND_INT_DISABLE);
            let allocator = if bar_info.is_mmio {
                if bar_info.is_64bit {
                    upstream.mmio_hi_regions()
                } else {
                    upstream.mmio_lo_regions()
                }
            } else {
                upstream.pio_regions()
            };
            let addr_mask: u32 = if bar_info.is_mmio {
                PCI_BAR_MMIO_ADDR_MASK
            } else {
                PCI_BAR_PIO_ADDR_MASK
            };
            let is_io_space = PCIE_HAS_IO_ADDR_SPACE && bar_info.is_mmio;
            let align_size = if bar_info.size as usize >= PAGE_SIZE || is_io_space {
                bar_info.size as usize
            } else {
                PAGE_SIZE
            };
            let alloc1 = allocator.lock().allocate_by_size(align_size, align_size);
            match alloc1 {
                Some(a) => bar_info.allocation = Some(a),
                None => {
                    if bar_info.is_mmio && bar_info.is_64bit {
                        bar_info.allocation = upstream
                            .mmio_lo_regions()
                            .lock()
                            .allocate_by_size(align_size, align_size);
                    }
                    if bar_info.allocation.is_none() {
                        return Err(ZxError::NOT_FOUND);
                    }
                }
            }
            let bar_reg = bar_info.first_bar_reg;
            bar_info.bus_addr = bar_info.allocation.as_ref().unwrap().0 as u64;
            let cfg = self.cfg.as_ref().unwrap();
            let bar_val = cfg.read_bar(bar_reg) & !addr_mask;
            cfg.write_bar(bar_reg, (bar_info.bus_addr & 0xFFFF_FFFF) as u32 | bar_val);
            if bar_info.is_64bit {
                cfg.write_bar(bar_reg + 1, (bar_info.bus_addr >> 32) as u32);
            }
        }
        Ok(())
    }

    fn assign_cmd(&self, value: u16) {
        self.modify_cmd(0xffff, value)
    }

    fn modify_cmd(&self, clr: u16, set: u16) {
        let _cmd_lock = self.command_lock.lock();
        let cfg = self.cfg.as_ref().unwrap();
        let oldval = cfg.read16(PciReg16::Command);
        cfg.write16(PciReg16::Command, oldval & !clr | set)
    }
    fn modify_cmd_adv(&self, clr: u16, set: u16) -> ZxResult {
        if !self.inner.lock().plugged_in {
            return Err(ZxError::UNAVAILABLE);
        }
        let _guard = self.dev_lock.lock();
        self.modify_cmd(clr & !(1 << 10), set & !(1 << 10));
        Ok(())
    }
    pub fn upstream(&self) -> Weak<dyn IPciNode> {
        self.inner.lock().upstream.clone()
    }
    pub fn dev_id(&self) -> usize {
        self.dev_id
    }
    pub fn func_id(&self) -> usize {
        self.func_id
    }
    pub fn set_upstream(&self, up: Weak<dyn IPciNode>) {
        self.inner.lock().upstream = up;
    }
    pub fn set_super(&self, sup: Weak<dyn IPciNode>) {
        self.inner.lock().weak_super = sup;
    }
    fn pcie_device_type(&self) -> PcieDeviceType {
        for cap in self.inner.lock().caps.iter() {
            if let PciCapability::Pcie(_std, pcie) = cap {
                return pcie.dev_type;
            }
        }
        PcieDeviceType::Unknown
    }
    pub fn config(&self) -> Option<Arc<PciConfig>> {
        self.cfg.clone()
    }

    /// Enable MMIO.
    pub fn enable_mmio(&self, enable: bool) -> ZxResult {
        self.modify_cmd_adv(
            if enable { 0 } else { PCI_COMMAND_MEM_EN },
            if enable { PCI_COMMAND_MEM_EN } else { 0 },
        )
    }

    /// Enable PIO.
    pub fn enable_pio(&self, enable: bool) -> ZxResult {
        self.modify_cmd_adv(
            if enable { 0 } else { PCI_COMMAND_IO_EN },
            if enable { PCI_COMMAND_IO_EN } else { 0 },
        )
    }

    /// Enable bus mastering.
    pub fn enable_master(&self, enable: bool) -> ZxResult {
        self.modify_cmd_adv(
            if enable { 0 } else { PCI_COMMAND_BUS_MASTER_EN },
            if enable { PCI_COMMAND_BUS_MASTER_EN } else { 0 },
        )?;
        if let Some(up) = self.upstream().upgrade() {
            up.enable_bus_master(enable)
        } else {
            Ok(())
        }
    }

    /// Enable an IRQ.
    pub fn enable_irq(&self, irq_id: usize, enable: bool) {
        let _dev_lcok = self.dev_lock.lock();
        let inner = self.inner.lock();
        assert!(inner.plugged_in);
        assert!(irq_id < inner.irq.handlers.len());
        if enable {
            assert!(!inner.disabled);
            assert!(inner.irq.handlers[irq_id].has_handler());
        }
        match inner.irq.mode {
            PcieIrqMode::Legacy => {
                if enable {
                    self.modify_cmd(PCIE_CFG_COMMAND_INT_DISABLE, 0);
                } else {
                    self.modify_cmd(0, PCIE_CFG_COMMAND_INT_DISABLE);
                }
            }
            PcieIrqMode::Msi => {
                let (_std, msi) = inner.msi().unwrap();
                if msi.has_pvm {
                    let mut val = self
                        .cfg
                        .as_ref()
                        .unwrap()
                        .read32_offset(msi.mask_bits_offset);
                    if enable {
                        val &= !(1 >> irq_id);
                    } else {
                        val |= 1 << irq_id;
                    }
                    self.cfg
                        .as_ref()
                        .unwrap()
                        .write32_offset(msi.mask_bits_offset, val);
                }
                // x86_64 does not support msi masking
                #[cfg(not(target_arch = "x86_64"))]
                error!("If the platform supports msi masking, do so");
            }
            _ => {
                unreachable!();
            }
        }
        inner.irq.handlers[irq_id].enable(enable);
    }

    /// Register an IRQ handle.
    pub fn register_irq_handle(&self, irq_id: usize, handle: Box<dyn Fn() -> u32 + Send + Sync>) {
        let _dev_lcok = self.dev_lock.lock();
        let inner = self.inner.lock();
        assert!(!inner.disabled);
        assert!(inner.plugged_in);
        assert!(inner.irq.mode != PcieIrqMode::Disabled);
        assert!(irq_id < inner.irq.handlers.len());
        inner.irq.handlers[irq_id].set_handler(Some(handle));
    }

    /// Unregister an IRQ handle.
    pub fn unregister_irq_handle(&self, irq_id: usize) {
        let _dev_lcok = self.dev_lock.lock();
        let inner = self.inner.lock();
        assert!(!inner.disabled);
        assert!(inner.plugged_in);
        assert!(inner.irq.mode != PcieIrqMode::Disabled);
        assert!(irq_id < inner.irq.handlers.len());
        inner.irq.handlers[irq_id].set_handler(None);
    }

    /// Get PcieBarInfo.
    pub fn get_bar(&self, bar_num: usize) -> Option<PcieBarInfo> {
        if bar_num >= self.bar_count {
            None
        } else {
            Some(self.inner.lock().bars[bar_num])
        }
    }

    /// Gets info about the capabilities of a PCI device's IRQ modes.
    pub fn get_irq_mode_capabilities(&self, mode: PcieIrqMode) -> ZxResult<PcieIrqModeCaps> {
        let inner = self.inner.lock();
        if inner.plugged_in {
            match mode {
                PcieIrqMode::Disabled => Ok(PcieIrqModeCaps::default()),
                PcieIrqMode::Legacy => {
                    if inner.irq.legacy.pin != 0 {
                        Ok(PcieIrqModeCaps {
                            max_irqs: 1,
                            per_vector_masking_supported: true,
                        })
                    } else {
                        warn!("get_irq_mode_capabilities: Legacy pin == 0");
                        Err(ZxError::NOT_SUPPORTED)
                    }
                }
                PcieIrqMode::Msi => {
                    if let Some((_std, msi)) = inner.msi() {
                        return Ok(PcieIrqModeCaps {
                            max_irqs: msi.max_irq,
                            per_vector_masking_supported: msi.has_pvm,
                        });
                    }
                    warn!("get_irq_mode_capabilities: MSI not found");
                    Err(ZxError::NOT_SUPPORTED)
                }
                PcieIrqMode::MsiX => Err(ZxError::NOT_SUPPORTED),
                _ => Err(ZxError::INVALID_ARGS),
            }
        } else {
            Err(ZxError::BAD_STATE)
        }
    }
    fn mask_legacy_irq(&self, inner: &MutexGuard<PcieDeviceInner>, mask: bool) -> ZxResult {
        if (**inner).irq.handlers.is_empty() {
            return Err(ZxError::INVALID_ARGS);
        }
        if mask {
            self.modify_cmd(0, PCIE_CFG_COMMAND_INT_DISABLE);
        } else {
            self.modify_cmd(PCIE_CFG_COMMAND_INT_DISABLE, 0);
        }
        (**inner).irq.handlers[0].set_masked(mask);
        Ok(())
    }
    fn reset_irq_bookkeeping(&self, inner: &mut MutexGuard<PcieDeviceInner>) {
        inner.irq.handlers.clear();
        inner.irq.mode = PcieIrqMode::Disabled;
        inner.irq.registered_handler_count = 0;
    }
    fn allocate_irq_handler(
        &self,
        inner: &mut MutexGuard<PcieDeviceInner>,
        requested_irqs: usize,
        masked: bool,
    ) {
        assert!(inner.irq.handlers.is_empty());
        for i in 0..requested_irqs {
            inner.irq.handlers.push(Arc::new(PcieIrqHandlerState {
                irq_id: i,
                enabled: Mutex::new(false),
                masked: Mutex::new(masked),
                handler: Mutex::new(None),
            }))
        }
    }
    fn enter_msi_irq_mode(
        &self,
        inner: &mut MutexGuard<PcieDeviceInner>,
        requested_irqs: usize,
    ) -> ZxResult {
        let (_std, msi) = inner.msi().ok_or(ZxError::NOT_SUPPORTED)?;
        let initially_masked = if msi.has_pvm {
            self.cfg
                .as_ref()
                .unwrap()
                .write32_offset(msi.mask_bits_offset, u32::MAX);
            true
        } else {
            false
        };
        match PciMsiBlock::allocate(requested_irqs) {
            Ok(block) => *msi.irq_block.lock() = block,
            Err(ex) => {
                self.leave_msi_irq_mode(inner);
                return Err(ex);
            }
        };
        self.allocate_irq_handler(inner, requested_irqs, initially_masked);
        inner.irq.mode = PcieIrqMode::Msi;
        let (_std, msi) = inner.msi().ok_or(ZxError::NOT_SUPPORTED)?;
        let block = msi.irq_block.lock();
        let (target_addr, target_data) = (block.target_addr, block.target_data);
        self.set_msi_target(inner, target_addr, target_data);
        self.set_msi_multi_message_enb(inner, requested_irqs);
        for (i, e) in inner.irq.handlers.iter().enumerate() {
            let arc_self = inner.arc_self();
            let handler_copy = e.clone();
            block.register_handler(
                i,
                Box::new(move || Self::msi_irq_handler(arc_self.clone(), handler_copy.clone())),
            );
        }
        self.set_msi_enb(inner, true);
        Ok(())
    }
    fn leave_msi_irq_mode(&self, inner: &mut MutexGuard<PcieDeviceInner>) {
        self.set_msi_target(inner, 0x0, 0x0);
        // free msi blocks
        {
            let (_std, msi) = inner.msi().unwrap();
            let block = msi.irq_block.lock();
            if block.allocated {
                for i in 0..block.num_irq {
                    block.register_handler(i, Box::new(|| {}));
                }
                block.free();
            }
        }
        self.reset_irq_bookkeeping(inner);
    }
    fn set_msi_target(
        &self,
        inner: &MutexGuard<PcieDeviceInner>,
        target_addr: u64,
        target_data: u32,
    ) {
        let (std, msi) = inner.msi().unwrap();
        assert!(msi.is_64bit || (target_addr >> 32) == 0);
        assert!((target_data >> 16) == 0);
        self.set_msi_enb(inner, false);
        self.mask_all_msi_vectors(inner);
        let cfg = self.cfg.as_ref().unwrap();
        let addr_reg = std.base + 0x4;
        let addr_reg_upper = std.base + 0x8;
        let data_reg = std.base + PciCapabilityMsi::addr_offset(msi.is_64bit) as u16;
        cfg.write32_(addr_reg as usize, target_addr as u32);
        if msi.is_64bit {
            cfg.write32_(addr_reg_upper as usize, (target_addr >> 32) as u32);
        }
        cfg.write16_(data_reg as usize, target_data as u16);
    }
    fn set_msi_multi_message_enb(
        &self,
        inner: &MutexGuard<PcieDeviceInner>,
        requested_irqs: usize,
    ) {
        assert!((1..=PCIE_MAX_MSI_IRQS).contains(&requested_irqs));
        let log2 = requested_irqs.next_power_of_two().trailing_zeros();
        assert!(log2 <= 5);
        let cfg = self.cfg.as_ref().unwrap();
        let (std, _msi) = inner.msi().unwrap();
        let ctrl_addr = std.base as usize + PciCapabilityMsi::ctrl_offset();
        let mut val = cfg.read16_(ctrl_addr);
        val = (val & !0x70) | ((log2 as u16 & 0x7) << 4);
        cfg.write16_(ctrl_addr, val);
    }
    fn set_msi_enb(&self, inner: &MutexGuard<PcieDeviceInner>, enable: bool) {
        let cfg = self.cfg.as_ref().unwrap();
        let (std, _msi) = inner.msi().unwrap();
        let ctrl_addr = std.base as usize + PciCapabilityMsi::ctrl_offset();
        let val = cfg.read16_(ctrl_addr);
        cfg.write16_(ctrl_addr, (val & !0x1) | (enable as u16));
    }
    fn mask_all_msi_vectors(&self, inner: &MutexGuard<PcieDeviceInner>) {
        for i in 0..inner.irq.handlers.len() {
            self.mask_msi_irq(inner, i, true);
        }
        // just to be careful
        let cfg = self.cfg.as_ref().unwrap();
        let (_std, msi) = inner.msi().unwrap();
        if msi.has_pvm {
            cfg.write32_offset(msi.mask_bits_offset, u32::MAX);
        }
    }
    fn mask_msi_irq(&self, inner: &MutexGuard<PcieDeviceInner>, irq: usize, mask: bool) -> bool {
        assert!(!inner.irq.handlers.is_empty());
        let cfg = self.cfg.as_ref().unwrap();
        let (_std, msi) = inner.msi().unwrap();
        if mask && !msi.has_pvm {
            return false;
        }
        if msi.has_pvm {
            assert!(irq < PCIE_MAX_MSI_IRQS);
            let addr = msi.mask_bits_offset;
            let mut val = cfg.read32_offset(addr);
            if mask {
                val |= 1 << irq;
            } else {
                val &= !(1 << irq);
            }
            cfg.write32_offset(addr, val);
        }
        let ret = inner.irq.handlers[0].get_masked();
        inner.irq.handlers[0].set_masked(mask);
        ret
    }
    fn msi_irq_handler(dev: Arc<PcieDevice>, state: Arc<PcieIrqHandlerState>) {
        // Perhaps dead lock?
        let inner = dev.inner.lock();
        let (_std, msi) = inner.msi().unwrap();
        if msi.has_pvm && dev.mask_msi_irq(&inner, state.irq_id, true) {
            return;
        }
        if let Some(h) = &*state.handler.lock() {
            let ret = h();
            if (ret & PCIE_IRQRET_MASK) == 0 {
                dev.mask_msi_irq(&inner, state.irq_id, false);
            }
        }
    }

    /// Set IRQ mode.
    pub fn set_irq_mode(&self, mode: PcieIrqMode, requested_irqs: usize) -> ZxResult {
        let mut inner = self.inner.lock();
        let mut requested_irqs = requested_irqs;
        if let PcieIrqMode::Disabled = mode {
            requested_irqs = 0;
        } else if !inner.plugged_in {
            return Err(ZxError::BAD_STATE);
        } else if requested_irqs < 1 {
            return Err(ZxError::INVALID_ARGS);
        }
        match inner.irq.mode {
            PcieIrqMode::Legacy => {
                self.mask_legacy_irq(&inner, true)?;
                inner
                    .irq
                    .legacy
                    .shared_handler
                    .remove_device(inner.arc_self());
                self.reset_irq_bookkeeping(&mut inner);
            }
            PcieIrqMode::Msi => {
                self.leave_msi_irq_mode(&mut inner);
            }
            PcieIrqMode::MsiX => {
                return Err(ZxError::NOT_SUPPORTED);
            }
            PcieIrqMode::Disabled => {}
            _ => {
                return Err(ZxError::INVALID_ARGS);
            }
        }
        match mode {
            PcieIrqMode::Disabled => Ok(()),
            PcieIrqMode::Legacy => {
                if inner.irq.legacy.pin == 0 || requested_irqs > 1 {
                    return Err(ZxError::NOT_SUPPORTED);
                }
                self.modify_cmd(0, PCIE_CFG_COMMAND_INT_DISABLE);
                self.allocate_irq_handler(&mut inner, requested_irqs, true);
                inner.irq.mode = PcieIrqMode::Legacy;
                inner.irq.legacy.shared_handler.add_device(inner.arc_self());
                Ok(())
            }
            PcieIrqMode::Msi => self.enter_msi_irq_mode(&mut inner, requested_irqs),
            PcieIrqMode::MsiX => Err(ZxError::NOT_SUPPORTED),
            _ => Err(ZxError::INVALID_ARGS),
        }
    }

    /// Read the device's config.
    pub fn config_read(&self, offset: usize, width: usize) -> ZxResult<u32> {
        let inner = self.inner.lock();
        let cfg_size: usize = if inner.pcie().is_some() {
            PCIE_BASE_CONFIG_SIZE
        } else {
            PCIE_EXTENDED_CONFIG_SIZE
        };
        if offset + width > cfg_size {
            return Err(ZxError::INVALID_ARGS);
        }
        match width {
            1 => Ok(self.cfg.as_ref().unwrap().read8_offset(offset) as u32),
            2 => Ok(self.cfg.as_ref().unwrap().read16_offset(offset) as u32),
            4 => Ok(self.cfg.as_ref().unwrap().read32_offset(offset) as u32),
            _ => Err(ZxError::INVALID_ARGS),
        }
    }

    /// Write the device's config.
    pub fn config_write(&self, offset: usize, width: usize, val: u32) -> ZxResult {
        let inner = self.inner.lock();
        let cfg_size: usize = if inner.pcie().is_some() {
            PCIE_BASE_CONFIG_SIZE
        } else {
            PCIE_EXTENDED_CONFIG_SIZE
        };
        if offset + width > cfg_size {
            return Err(ZxError::INVALID_ARGS);
        }
        match width {
            1 => self.cfg.as_ref().unwrap().write8_offset(offset, val as u8),
            2 => self
                .cfg
                .as_ref()
                .unwrap()
                .write16_offset(offset, val as u16),
            4 => self
                .cfg
                .as_ref()
                .unwrap()
                .write32_offset(offset, val as u32),
            _ => return Err(ZxError::INVALID_ARGS),
        };
        Ok(())
    }
}

#[derive(PartialEq, Eq)]
pub enum PciNodeType {
    Root,
    Bridge,
    Device,
}

pub trait IPciNode: Send + Sync {
    fn node_type(&self) -> PciNodeType;
    fn device(&self) -> Arc<PcieDevice>;
    fn as_upstream(&self) -> Option<Arc<PcieUpstream>>;
    fn as_root(&self) -> Option<&PciRoot> {
        None
    }
    fn allocate_bars(&self) -> ZxResult {
        unimplemented!("IPciNode.allocate_bars")
    }
    fn disable(&self) {
        unimplemented!("IPciNode.disable");
    }
    fn pf_mmio_regions(&self) -> Arc<Mutex<RegionAllocator>> {
        unimplemented!("IPciNode.pf_mmio_regions");
    }
    fn mmio_lo_regions(&self) -> Arc<Mutex<RegionAllocator>> {
        unimplemented!("IPciNode.mmio_lo_regions");
    }
    fn mmio_hi_regions(&self) -> Arc<Mutex<RegionAllocator>> {
        unimplemented!("IPciNode.mmio_hi_regions");
    }
    fn pio_regions(&self) -> Arc<Mutex<RegionAllocator>> {
        unimplemented!("IPciNode.pio_regions");
    }
    fn enable_bus_master(&self, _enable: bool) -> ZxResult {
        unimplemented!("IPciNode.enable_bus_master");
    }
    fn enable_irq(&self, irq_id: usize) {
        self.device().enable_irq(irq_id, true);
    }
    fn disable_irq(&self, irq_id: usize) {
        self.device().enable_irq(irq_id, false);
    }
    fn register_irq_handle(&self, irq_id: usize, handle: Box<dyn Fn() -> u32 + Send + Sync>) {
        self.device().register_irq_handle(irq_id, handle);
    }
    fn unregister_irq_handle(&self, irq_id: usize) {
        self.device().unregister_irq_handle(irq_id);
    }
}

pub struct PciRoot {
    pub base_upstream: Arc<PcieUpstream>,
    lut: PciIrqSwizzleLut,
    mmio_hi: Arc<Mutex<RegionAllocator>>,
    mmio_lo: Arc<Mutex<RegionAllocator>>,
    pio_region: Arc<Mutex<RegionAllocator>>,
}

impl PciRoot {
    pub fn new(managed_bus_id: usize, lut: PciIrqSwizzleLut, bus: &PCIeBusDriver) -> Arc<Self> {
        let inner_ups = PcieUpstream::create(managed_bus_id);
        let node = Arc::new(PciRoot {
            base_upstream: inner_ups,
            lut,
            mmio_hi: bus.mmio_hi.clone(),
            mmio_lo: bus.mmio_lo.clone(),
            pio_region: bus.pio_region.clone(),
        });
        node.base_upstream
            .set_super(Arc::downgrade(&(node.clone() as _)));
        node
    }
    pub fn swizzle(&self, dev_id: usize, func_id: usize, pin: usize) -> ZxResult<usize> {
        self.lut.swizzle(dev_id, func_id, pin)
    }
    pub fn managed_bus_id(&self) -> usize {
        self.base_upstream.managed_bus_id
    }
}

impl IPciNode for PciRoot {
    fn node_type(&self) -> PciNodeType {
        PciNodeType::Root
    }
    fn device(&self) -> Arc<PcieDevice> {
        unimplemented!()
    }
    fn as_upstream(&self) -> Option<Arc<PcieUpstream>> {
        Some(self.base_upstream.clone())
    }
    fn as_root(&self) -> Option<&PciRoot> {
        Some(self)
    }
    fn allocate_bars(&self) -> ZxResult {
        unimplemented!();
    }
    fn mmio_lo_regions(&self) -> Arc<Mutex<RegionAllocator>> {
        self.mmio_lo.clone()
    }
    fn mmio_hi_regions(&self) -> Arc<Mutex<RegionAllocator>> {
        self.mmio_hi.clone()
    }
    fn pio_regions(&self) -> Arc<Mutex<RegionAllocator>> {
        self.pio_region.clone()
    }
    fn enable_bus_master(&self, _enable: bool) -> ZxResult {
        Ok(())
    }
}

pub struct PciDeviceNode {
    base_device: Arc<PcieDevice>,
}

impl PciDeviceNode {
    pub fn new(
        upstream: Weak<dyn IPciNode>,
        dev_id: usize,
        func_id: usize,
        driver: &PCIeBusDriver,
    ) -> Option<Arc<Self>> {
        info!("Create PciDeviceNode");
        let up_to_move = upstream.clone();
        PcieDevice::create(upstream, dev_id, func_id, driver).map(move |x| {
            let node = Arc::new(PciDeviceNode { base_device: x });
            node.base_device
                .as_ref()
                .set_super(Arc::downgrade(&(node.clone() as _)));
            // test_interface(node.clone() as _);
            driver.link_device_to_upstream(node.clone(), up_to_move.clone());
            node
        })
    }
}

impl IPciNode for PciDeviceNode {
    fn node_type(&self) -> PciNodeType {
        PciNodeType::Device
    }
    fn device(&self) -> Arc<PcieDevice> {
        self.base_device.clone()
    }
    fn as_upstream(&self) -> Option<Arc<PcieUpstream>> {
        None
    }
    fn allocate_bars(&self) -> ZxResult {
        self.base_device.allocate_bars()
    }
    fn enable_bus_master(&self, enable: bool) -> ZxResult {
        self.base_device.enable_master(enable)
    }
}

pub struct PciBridge {
    base_device: Arc<PcieDevice>,
    base_upstream: Arc<PcieUpstream>,
    mmio_lo: Arc<Mutex<RegionAllocator>>,
    mmio_hi: Arc<Mutex<RegionAllocator>>,
    pio_region: Arc<Mutex<RegionAllocator>>,
    pf_mmio: Arc<Mutex<RegionAllocator>>,
    inner: Mutex<PciBridgeInner>,
    downstream_bus_mastering_cnt: Mutex<usize>,
}

#[derive(Default)]
struct PciBridgeInner {
    pf_mem_base: u64,
    pf_mem_limit: u64,
    mem_base: u32,
    mem_limit: u32,
    io_base: u32,
    io_limit: u32,
    supports_32bit_pio: bool,
}

impl PciBridge {
    pub fn new(
        upstream: Weak<dyn IPciNode>,
        dev_id: usize,
        func_id: usize,
        managed_bus_id: usize,
        driver: &PCIeBusDriver,
    ) -> Option<Arc<Self>> {
        warn!("Create Pci Bridge");
        let father = upstream.upgrade().and_then(|x| x.as_upstream());
        father.as_ref()?;
        let inner_ups = PcieUpstream::create(managed_bus_id);
        let inner_dev = PcieDevice::create(upstream, dev_id, func_id, driver);
        inner_dev.map(move |x| {
            let node = Arc::new(PciBridge {
                base_device: x,
                base_upstream: inner_ups,
                mmio_hi: Default::default(),
                mmio_lo: Default::default(),
                pf_mmio: Default::default(),
                pio_region: Default::default(),
                inner: Default::default(),
                downstream_bus_mastering_cnt: Mutex::new(0),
            });
            node.base_device
                .set_super(Arc::downgrade(&(node.clone() as _)));
            node.base_upstream
                .set_super(Arc::downgrade(&(node.clone() as _)));
            node.init(driver);
            node
        })
    }

    fn init(&self, driver: &PCIeBusDriver) {
        let device = self.base_device.clone();
        let as_upstream = self.base_upstream.clone();
        let cfg = device.cfg.as_ref().unwrap();
        let primary_id = cfg.read8(PciReg8::PrimaryBusId) as usize;
        let secondary_id = cfg.read8(PciReg8::SecondaryBusId) as usize;
        assert_ne!(primary_id, secondary_id);
        assert_eq!(primary_id, device.bus_id);
        assert_eq!(secondary_id, as_upstream.managed_bus_id);

        let base: u32 = cfg.read8(PciReg8::IoBase) as _;
        let limit: u32 = cfg.read8(PciReg8::IoLimit) as _;
        let mut inner = self.inner.lock();
        inner.supports_32bit_pio = ((base & 0xF) == 0x1) && ((base & 0xF) == (limit & 0xF));
        inner.io_base = (base & !0xF) << 8;
        inner.io_limit = limit << 8 | 0xFFF;
        if inner.supports_32bit_pio {
            inner.io_base |= (cfg.read16(PciReg16::IoBaseUpper) as u32) << 16;
            inner.io_limit |= (cfg.read16(PciReg16::IoLimitUpper) as u32) << 16;
        }
        inner.mem_base = (cfg.read16(PciReg16::MemoryBase) as u32) << 16 & !0xFFFFF;
        inner.mem_limit = (cfg.read16(PciReg16::MemoryLimit) as u32) << 16 | 0xFFFFF;

        let base: u64 = cfg.read16(PciReg16::PrefetchableMemoryBase) as _;
        let limit: u64 = cfg.read16(PciReg16::PrefetchableMemoryLimit) as _;
        let supports_64bit_pf_mem = ((base & 0xF) == 0x1) && ((base & 0xF) == (limit & 0xF));
        inner.pf_mem_base = (base & !0xF) << 16;
        inner.pf_mem_limit = (limit << 16) | 0xFFFFF;
        if supports_64bit_pf_mem {
            inner.pf_mem_base |= (cfg.read32(PciReg32::PrefetchableMemoryBaseUpper) as u64) << 32;
            inner.pf_mem_limit |= (cfg.read32(PciReg32::PrefetchableMemoryLimitUpper) as u64) << 32;
        }

        device.inner.lock().plugged_in = true;
        let sup = device.inner.lock().weak_super.upgrade().unwrap();
        let upstream = device.upstream();
        driver.link_device_to_upstream(sup, upstream);
        as_upstream.scan_downstream(driver);
    }
}

impl IPciNode for PciBridge {
    fn node_type(&self) -> PciNodeType {
        PciNodeType::Bridge
    }
    fn device(&self) -> Arc<PcieDevice> {
        self.base_device.clone()
    }
    fn as_upstream(&self) -> Option<Arc<PcieUpstream>> {
        Some(self.base_upstream.clone())
    }
    fn pf_mmio_regions(&self) -> Arc<Mutex<RegionAllocator>> {
        self.pf_mmio.clone()
    }
    fn mmio_lo_regions(&self) -> Arc<Mutex<RegionAllocator>> {
        self.mmio_lo.clone()
    }
    fn mmio_hi_regions(&self) -> Arc<Mutex<RegionAllocator>> {
        self.mmio_hi.clone()
    }
    fn pio_regions(&self) -> Arc<Mutex<RegionAllocator>> {
        self.pio_region.clone()
    }
    fn allocate_bars(&self) -> ZxResult {
        warn!("Allocate bars for bridge");
        let inner = self.inner.lock();
        let upstream = self.base_device.upstream().upgrade().unwrap();
        if inner.io_base <= inner.io_limit {
            let size = (inner.io_limit - inner.io_base + 1) as usize;
            if !upstream
                .pio_regions()
                .lock()
                .allocate_by_addr(inner.io_base as usize, size)
            {
                return Err(ZxError::NO_MEMORY);
            }
            self.pio_regions().lock().add(inner.io_base as usize, size);
        }
        if inner.mem_base <= inner.mem_limit {
            let size = (inner.mem_limit - inner.mem_base + 1) as usize;
            if !upstream
                .mmio_lo_regions()
                .lock()
                .allocate_by_addr(inner.mem_base as usize, size)
            {
                return Err(ZxError::NO_MEMORY);
            }
            self.mmio_lo_regions()
                .lock()
                .add(inner.mem_base as usize, size);
        }
        if inner.pf_mem_base <= inner.pf_mem_limit {
            let size = (inner.pf_mem_limit - inner.pf_mem_base + 1) as usize;
            match upstream.node_type() {
                PciNodeType::Root => {
                    if !upstream
                        .mmio_lo_regions()
                        .lock()
                        .allocate_by_addr(inner.pf_mem_base as usize, size)
                        && !upstream
                            .mmio_hi_regions()
                            .lock()
                            .allocate_by_addr(inner.pf_mem_base as usize, size)
                    {
                        return Err(ZxError::NO_MEMORY);
                    }
                }
                PciNodeType::Bridge => {
                    if !upstream
                        .pf_mmio_regions()
                        .lock()
                        .allocate_by_addr(inner.pf_mem_base as usize, size)
                    {
                        return Err(ZxError::NO_MEMORY);
                    }
                }
                _ => {
                    unreachable!("Upstream node must be root or bridge");
                }
            }
            self.pf_mmio_regions()
                .lock()
                .add(inner.pf_mem_base as usize, size);
        }
        self.base_device.allocate_bars()?;
        warn!("Allocate finish");
        upstream.as_upstream().unwrap().allocate_downstream_bars();
        Ok(())
    }
    fn enable_bus_master(&self, enable: bool) -> ZxResult {
        let count = {
            let mut count = self.downstream_bus_mastering_cnt.lock();
            if enable {
                *count += 1;
            } else if *count == 0 {
                return Err(ZxError::BAD_STATE);
            } else {
                *count -= 1;
            }
            *count
        };
        if count > 0 {
            self.base_device.enable_master(false)?;
        }
        if count == 1 && enable {
            self.base_device.enable_master(true)?;
        }
        Ok(())
    }
}

const PCI_HEADER_TYPE_MULTI_FN: u8 = 0x80;
const PCI_HEADER_TYPE_STANDARD: u8 = 0x00;
const PCI_HEADER_TYPE_PCI_BRIDGE: u8 = 0x01;

const PCI_BAR_IO_TYPE_MASK: u32 = 0x1;
const PCI_BAR_IO_TYPE_MMIO: u32 = 0x0;
const PCI_BAR_IO_TYPE_PIO: u32 = 0x1;

const PCI_BAR_MMIO_TYPE_MASK: u32 = 0x6;
const PCI_BAR_MMIO_TYPE_32BIT: u32 = 0x0;
const PCI_BAR_MMIO_TYPE_64BIT: u32 = 0x4;
const PCI_BAR_MMIO_ADDR_MASK: u32 = 0xFFFF_FFF0;
const PCI_BAR_PIO_ADDR_MASK: u32 = 0xFFFF_FFFC;

const PCI_BAR_MMIO_PREFETCH_MASK: u32 = 0x8;

const PCI_COMMAND_IO_EN: u16 = 0x0001;
const PCI_COMMAND_MEM_EN: u16 = 0x0002;
const PCI_COMMAND_BUS_MASTER_EN: u16 = 0x0004;

const PCIE_CFG_COMMAND_INT_DISABLE: u16 = 1 << 10;
const PCIE_CFG_STATUS_INT_SYS: u16 = 1 << 3;

#[cfg(target_arch = "x86_64")]
const PCIE_HAS_IO_ADDR_SPACE: bool = true;
#[cfg(not(target_arch = "x86_64"))]
const PCIE_HAS_IO_ADDR_SPACE: bool = false;

/// A structure used to hold output parameters when calling
/// `pcie_query_irq_mode_capabilities`.
#[derive(Default)]
pub struct PcieIrqModeCaps {
    /// The maximum number of IRQ supported by the selected mode
    pub max_irqs: u32,
    /// For MSI or MSI-X, indicates whether or not per-vector-masking has been
    /// implemented by the hardware.
    pub per_vector_masking_supported: bool,
}
