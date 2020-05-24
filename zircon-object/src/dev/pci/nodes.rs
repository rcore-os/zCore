use super::*;
// use ::pci::*;
use alloc::sync::*;
use alloc::vec::Vec;
use spin::{Mutex, MutexGuard};

pub struct PcieUpstream {
    pub driver: Weak<PCIeBusDriver>,
    pub managed_bus_id: usize,
    downstream: [Option<Arc<PcieDevice>>; PCI_MAX_FUNCTIONS_PER_BUS],
    weak_self: Weak<Self>,
    weak_super: Weak<dyn IPciNode + Send + Sync>,
}

impl PcieUpstream {
    pub fn driver(&self) -> Weak<PCIeBusDriver> {
        self.driver.clone()
    }
    pub fn create(driver: Weak<PCIeBusDriver>, managed_bus_id: usize) -> Arc<Self> {
        let ret = Arc::new(PcieUpstream {
            driver,
            managed_bus_id,
            downstream: [None; PCI_MAX_FUNCTIONS_PER_BUS],
            weak_self: Weak::new(),
            weak_super: Weak::<PciRoot>::new(),
        });
        ret.weak_self = Arc::downgrade(&ret);
        ret
    }
    pub fn scan_downstream(&mut self) {
        for dev_id in 0..PCI_MAX_DEVICES_PER_BUS {
            for func_id in 0..PCI_MAX_FUNCTIONS_PER_DEVICE {
                let cfg =
                    self.driver
                        .upgrade()
                        .unwrap()
                        .get_config(self.managed_bus_id, dev_id, func_id);
                if cfg.is_none() {
                    warn!("bus being scanned is outside ecam region!\n");
                    return;
                }
                let (cfg, paddr) = cfg.unwrap();
                let vendor_id = cfg.read16(PciReg16::VendorId);
                let mut good_device = vendor_id as usize != PCIE_INVALID_VENDOR_ID;
                if good_device {
                    let device_id = cfg.read16(PciReg16::DeviceId);
                    info!(
                        "Found device {:#x?}:{:#x?} at {:#x?}:{:#x?}.{:#x?}\n",
                        vendor_id, device_id, self.managed_bus_id, dev_id, func_id
                    );
                    let ndx = dev_id * PCI_MAX_FUNCTIONS_PER_DEVICE + func_id;
                    let downstream_device = self.get_downstream(ndx);
                    match downstream_device {
                        Some(dev) => {
                            // *************TODO:
                            // if dev.is_bridge() {
                            //     dev.scan_downstream();
                            // }
                        }
                        None => {
                            if let None =
                                self.scan_device(cfg.as_ref(), dev_id, func_id, Some(vendor_id))
                            {
                                info!(
                                    "failed to initialize device {:#x?}:{:#x?}.{:#x?}\n",
                                    self.managed_bus_id, dev_id, func_id
                                );
                                good_device = false;
                            }
                        }
                    }
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
    fn scan_device(
        &mut self,
        cfg: &PciConfig,
        dev_id: usize,
        func_id: usize,
        vendor_id: Option<u16>,
    ) -> Option<Arc<dyn IPciNode + Send + Sync>> {
        let vendor_id = vendor_id.or(Some(cfg.read16(PciReg16::VendorId))).unwrap();
        if vendor_id == PCIE_INVALID_VENDOR_ID as u16 {
            return None;
        }
        let header_type = cfg.read8(PciReg8::HeaderType) & 0x7f;
        if header_type == PCI_HEADER_TYPE_PCI_BRIDGE {
            let secondary_id = cfg.read8(PciReg8::SecondaryBusId);
            PciBridge::new(self.weak_super, dev_id, func_id, secondary_id as usize).map(|x| x as _)
        } else {
            PciDeviceNode::new(self.weak_super.clone(), dev_id, func_id).map(|x| x as _)
        }
    }
    fn get_downstream(&self, index: usize) -> Option<Arc<PcieDevice>> {
        if index >= PCI_MAX_FUNCTIONS_PER_BUS {
            return None;
        }
        self.downstream[index].clone()
    }
}

#[derive(Default, Copy, Clone)]
struct PcieBarInfo {
    is_mmio: bool,
    is_64bit: bool,
    is_prefetchable: bool,
    first_bar_reg: usize,
    size: u64,
    bus_addr: u64,
}
#[derive(Default)]
struct SharedLegacyIrqHandler {}
#[derive(Default)]
struct PcieLegacyIrqState {
    pub pin: u8,
    pub id: usize,
    pub shared_handler: SharedLegacyIrqHandler,
}
// #[derive(Default)]
pub struct PcieDevice {
    driver: Weak<PCIeBusDriver>,
    managed_bus_id: usize,
    dev_id: usize,
    func_id: usize,
    plugged_in: bool,
    is_bridge: bool,
    bar_count: usize,
    cfg: Option<Arc<PciConfig>>,
    cfg_phys: usize,
    bars: [PcieBarInfo; 6],
    caps: Vec<PciCapacity>,
    irq: PcieLegacyIrqState,
    dev_lock: Mutex<()>,
    weak_self: Weak<Self>,
    weak_super: Weak<(dyn IPciNode + Send + Sync)>,
    upstream: Weak<(dyn IPciNode + Send + Sync)>,
    vendor_id: u16,
    device_id: u16,
    class_id: u8,
    subclass_id: u8,
    prog_if: u8,
    rev_id: u8,
}

// impl Default for PcieDevice {
//     fn default() -> Self {
//         PcieDevice {
//             driver: Weak::default(),
//             mana
//         }
//     }
// }

impl PcieDevice {
    pub fn create(
        upstream: Weak<dyn IPciNode + Send + Sync>,
        dev_id: usize,
        func_id: usize,
    ) -> Option<Arc<Self>> {
        let ups = upstream.upgrade().unwrap().upstream();
        if let None = ups {
            return None;
        }
        let ups = ups.unwrap();
        let result = ups
            .driver
            .upgrade()
            .unwrap()
            .get_config(ups.managed_bus_id, dev_id, func_id);
        if let None = result {
            warn!("Failed to fetch config for device ");
            return None;
        }
        let (cfg, paddr) = result.unwrap();
        let inst = Arc::new(PcieDevice {
            driver: ups.driver,
            managed_bus_id: ups.managed_bus_id,
            dev_id,
            func_id,
            plugged_in: false,
            is_bridge: false,
            bar_count: 6, // PCIE BAR regs per device
            cfg: Some(cfg),
            cfg_phys: paddr,
            bars: [PcieBarInfo::default(); 6],
            caps: Vec::new(),
            irq: PcieLegacyIrqState::default(),
            dev_lock: Mutex::default(),
            weak_super: Weak::<PciRoot>::new(),
            upstream: Weak::<PciRoot>::new(),
            weak_self: Weak::new(),
            vendor_id: 0,
            device_id: 0,
            class_id: 0,
            subclass_id: 0,
            prog_if: 0,
            rev_id: 0,
        });
        inst.weak_self = Arc::downgrade(&inst);
        inst.init(upstream);
        Some(inst)
    }
    fn init(self: Arc<Self>, upstream: Weak<dyn IPciNode + Send + Sync>) -> ZxResult {
        let guard = self.dev_lock.lock();
        self.init_config(&guard, &upstream)?;
        self.plugged_in = true;
        self.driver
            .upgrade()
            .unwrap()
            .link_device_to_upstream(Arc::downgrade(&self), upstream);
        Ok(())
    }
    fn init_config(
        &mut self,
        lock: &MutexGuard<()>,
        upstream: &Weak<dyn IPciNode + Send + Sync>,
    ) -> ZxResult {
        let cfg = self.cfg.as_ref().unwrap();
        self.vendor_id = cfg.read16(PciReg16::VendorId);
        self.device_id = cfg.read16(PciReg16::DeviceId);
        self.class_id = cfg.read8(PciReg8::BaseClass);
        self.subclass_id = cfg.read8(PciReg8::SubClass);
        self.prog_if = cfg.read8(PciReg8::ProgramInterface);
        self.rev_id = cfg.read8(PciReg8::RevisionId);
        self.init_probe_bars(lock);
        self.init_capabilities(lock);
        self.init_legacy_irq(lock, upstream);
        Ok(())
    }
    fn init_probe_bars(&mut self, lock: &MutexGuard<()>) -> ZxResult {
        // probe bars
        let mut i = 0;
        let cfg = self.cfg.as_ref().unwrap();
        while i < self.bar_count {
            let bar_val = cfg.readBAR(i);
            let is_mmio = (bar_val & PCI_BAR_IO_TYPE_MASK) == PCI_BAR_IO_TYPE_MMIO;
            let is_64bit = is_mmio && (bar_val & PCI_BAR_MMIO_TYPE_MASK) == PCI_BAR_MMIO_TYPE_64BIT;
            if is_64bit {
                if i + 1 >= self.bar_count {
                    warn!(
                        "Illegal 64-bit MMIO BAR position {}/{} while fetching BAR info\n",
                        i, self.bar_count
                    );
                    return Err(ZxError::BAD_STATE);
                }
            } else {
                if is_mmio && ((bar_val & PCI_BAR_MMIO_TYPE_MASK) != PCI_BAR_MMIO_TYPE_32BIT) {
                    warn!(
                        "Unrecognized MMIO BAR type (BAR[{}] == {:#x?}) while fetching BAR info\n",
                        i, bar_val
                    );
                    return Err(ZxError::BAD_STATE);
                }
            }
            // Disable either MMIO or PIO (depending on the BAR type) access while we perform the probe.
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
            cfg.writeBAR(i, bar_val | addr_mask);
            let mut size_mask: u64 = !(cfg.readBAR(i) & addr_mask) as u64;
            cfg.writeBAR(i, bar_val);
            if is_mmio && is_64bit {
                let bar_id = i + 1;
                let bar_val = cfg.readBAR(bar_id);
                cfg.writeBAR(bar_id, 0xFFFF_FFFF);
                size_mask |= (!cfg.readBAR(bar_id) as u64) << 32;
                cfg.writeBAR(bar_id, bar_val);
            }
            let size = if is_mmio {
                size_mask + 1
            } else {
                (size_mask + 1) & PCIE_PIO_ADDR_SPACE_MASK
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
            };
            self.bars[i] = bar_info;
            i += 1;
            if is_64bit && bar_info.size > 0 {
                i += 1;
                if i >= self.bar_count {
                    return Err(ZxError::BAD_STATE);
                }
            }
        }
        Ok(())
    }
    fn init_capabilities(&mut self, lock: &MutexGuard<()>) -> ZxResult {
        let cfg = self.cfg.as_ref().unwrap();
        let mut cap_offset = cfg.read8(PciReg8::CapabilitiesPtr);
        let mut found_num = 0;
        while cap_offset != 0 && found_num < (256 - 64) / 4 {
            if cap_offset == 0xff || cap_offset < 64 || cap_offset > 256 - 4 {
                return Err(ZxError::INVALID_ARGS);
            }
            let id = cfg.read8_offset(cap_offset as usize);
            let cap = match id {
                0x5 => PciCapacity::Msi(cap_offset, id),
                0x10 => PciCapacity::Pcie(cap_offset, id),
                0x13 => PciCapacity::AdvFeatures(cap_offset, id),
                _ => PciCapacity::Std(cap_offset, id),
            };
            self.caps.push(cap);
            cap_offset = cfg.read8_offset(cap_offset as usize + 1) & 0xFC;
            found_num += 1;
        }
        Ok(())
    }
    fn init_legacy_irq(
        &mut self,
        lock: &MutexGuard<()>,
        upstream: &Weak<dyn IPciNode + Send + Sync>,
    ) -> ZxResult {
        self.modify_cmd(0, PCIE_CFG_COMMAND_INT_DISABLE);
        let cfg = self.cfg.as_ref().unwrap();
        let pin = cfg.read8(PciReg8::InterruptPin);
        self.irq.pin = pin;
        if pin != 0 {
            self.map_pin_to_irq(lock, upstream)?;
            self.irq.shared_handler = self.find_legacy_orq_handler(self.irq.id)?;
        }
        Ok(())
    }
    fn map_pin_to_irq(
        &mut self,
        lock: &MutexGuard<()>,
        upstream: &Weak<(dyn IPciNode + Send + Sync)>,
    ) -> ZxResult {
        if self.irq.pin == 0 || self.irq.pin > 4 {
            return Err(ZxError::BAD_STATE);
        }
        let pin = self.irq.pin - 1;
        let mut dev = self.weak_self.upgrade().unwrap();
        let mut upstream = upstream.clone();
        while let Some(up) = upstream.upgrade() {
            if let PciNodeType::Bridge = up.node_type() {
                let bups = up.upstream().unwrap();
                match bups.get_pcie_device_type() {
                    Unknown | SwitchUpstreamPort | PcieToPciBridge | PciToPcieBridge => {
                        pin = (pin + dev.dev_id as u8) % 4;
                    }
                    _ => (),
                }
                dev = up.device().unwrap();
                upstream = dev.upstream();
            } else {
                break;
            }
        }
        let upstream = upstream.upgrade();
        if let Some(up_ptr) = upstream {
            if let Some(up) = up_ptr.to_root() {
                return Ok(up.swizzle(dev.dev_id, dev.func_id, pin));
            }
        }
        Err(ZxError::BAD_STATE)
    }
    pub fn upstream(&self) -> Weak<dyn IPciNode + Send + Sync> {
        self.upstream.clone()
    }
}

pub enum PciNodeType {
    Root,
    Bridge,
    Device,
}

pub trait IPciNode {
    fn node_type(&self) -> PciNodeType;
    fn device(&mut self) -> Option<Arc<PcieDevice>>;
    fn upstream(&mut self) -> Option<Arc<PcieUpstream>>;
    fn to_root(&mut self) -> Option<&mut PciRoot>;
    fn to_device(&mut self) -> Option<&mut PciDeviceNode>;
    fn to_bridge(&mut self) -> Option<&mut PciBridge>;
}

pub enum PciCapacity {
    Msi(u8, u8),
    Pcie(u8, u8),
    AdvFeatures(u8, u8),
    Std(u8, u8),
}

pub struct PciRoot {
    base_upstream: Arc<PcieUpstream>,
    lut: Arc<dyn PcieRootSwizzle + Send + Sync>,
}

impl PciRoot {
    pub fn new(
        driver: Weak<PCIeBusDriver>,
        bus_id: usize,
        lut: Arc<PcieRootLUTSwizzle>,
    ) -> Arc<Self> {
        let inner_ups = PcieUpstream::create(driver, bus_id);
        let node = Arc::new(PciRoot {
            base_upstream: inner_ups,
            lut,
        });
        node.base_upstream.weak_super = Arc::downgrade(&(node as _));
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
    fn device(&mut self) -> Option<Arc<PcieDevice>> {
        None
    }
    fn upstream(&mut self) -> Option<Arc<PcieUpstream>> {
        Some(self.base_upstream.clone())
    }
    fn to_root(&mut self) -> Option<&mut PciRoot> {
        Some(self)
    }
    fn to_device(&mut self) -> Option<&mut PciDeviceNode> {
        None
    }
    fn to_bridge(&mut self) -> Option<&mut PciBridge> {
        None
    }
}

pub struct PciDeviceNode {
    base_device: Arc<PcieDevice>,
}

impl PciDeviceNode {
    pub fn new(
        upstream: Weak<dyn IPciNode + Send + Sync>,
        dev_id: usize,
        func_id: usize,
    ) -> Option<Arc<Self>> {
        PcieDevice::create(upstream, dev_id, func_id).map(|x| {
            let node = Arc::new(PciDeviceNode { base_device: x });
            node.base_device.weak_super = Arc::downgrade(&(node as _));
            test_interface(node);
            node
        })
    }
}

fn test_interface(t: Arc<(dyn IPciNode + Send + Sync)>) {}

impl IPciNode for PciDeviceNode {
    fn node_type(&self) -> PciNodeType {
        PciNodeType::Device
    }
    fn device(&mut self) -> Option<Arc<PcieDevice>> {
        Some(self.base_device.clone())
    }
    fn upstream(&mut self) -> Option<Arc<PcieUpstream>> {
        None
    }
    fn to_root(&mut self) -> Option<&mut PciRoot> {
        None
    }
    fn to_device(&mut self) -> Option<&mut PciDeviceNode> {
        Some(self)
    }
    fn to_bridge(&mut self) -> Option<&mut PciBridge> {
        None
    }
}

pub struct PciBridge {
    base_device: Arc<PcieDevice>,
    base_upstream: Arc<PcieUpstream>,
}

impl PciBridge {
    pub fn new(
        upstream: Weak<dyn IPciNode + Send + Sync>,
        dev_id: usize,
        func_id: usize,
        managed_bus_id: usize,
    ) -> Option<Arc<Self>> {
        let fa_ups = upstream.upgrade().and_then(|x| x.upstream());
        if fa_ups.is_none() {
            return None;
        }
        let inner_ups = PcieUpstream::create(fa_ups.unwrap().driver(), managed_bus_id);
        let inner_dev = PcieDevice::create(upstream, dev_id, func_id);
        inner_dev.map(move |x| {
            let node = Arc::new(PciBridge {
                base_device: x,
                base_upstream: inner_ups,
            });
            node.base_device.weak_super = Arc::downgrade(&(node as _));
            node.base_upstream.weak_super = Arc::downgrade(&(node as _));
            node
        })
    }
}

impl IPciNode for PciBridge {
    fn node_type(&self) -> PciNodeType {
        PciNodeType::Bridge
    }
    fn device(&mut self) -> Option<Arc<PcieDevice>> {
        Some(self.base_device.clone())
    }
    fn upstream(&mut self) -> Option<Arc<PcieUpstream>> {
        Some(self.base_upstream.clone())
    }
    fn to_root(&mut self) -> Option<&mut PciRoot> {
        None
    }
    fn to_device(&mut self) -> Option<&mut PciDeviceNode> {
        None
    }
    fn to_bridge(&mut self) -> Option<&mut PciBridge> {
        Some(self)
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
