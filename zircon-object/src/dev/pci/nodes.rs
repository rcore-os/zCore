use super::*;
use alloc::sync::*;
use spin::{Mutex, MutexGuard};

pub struct PcieUpstream {
    pub driver: Weak<PCIeBusDriver>,
    pub managed_bus_id: usize,
    downstream: [Arc<PcieDevice>; PCI_MAX_FUNCTIONS_PER_BUS],
    weak_self: Weak<Self>,
}

impl PcieUpstream {
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
                            if dev.is_bridge() {
                                dev.scan_downstream();
                            }
                        }
                        None => {
                            if let None = self.scan_device(cfg, dev_id, func_id, Some(vendor_id)) {
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
        vendor_id: Option<usize>,
    ) -> Option<PcieDevice> {
        let vendor_id = vendor_id
            .or(Some(cfg.read16(PciReg16::VendorId) as usize))
            .unwrap();
        if vendor_id == PCIE_INVALID_VENDOR_ID {
            return None;
        }
        let header_type = cfg.read8(PciReg8::HeaderType) & 0x7f;
        if header_type == PCI_HEADER_TYPE_PCI_BRIDGE {
            let secondary_id = cfg.read8(PciReg8::SecondaryBusId);
            Some(PcieBridge::create(
                self.weak_self.clone(),
                dev_id,
                func_id,
                secondary_id,
            ))
        } else {
            PcieDevice::create(self.weak_self.clone(), dev_id, func_id)
        }
    }
    fn get_downstream(&self, index: usize) -> Option<Arc<PcieDevice>> {
        if index >= PCI_MAX_FUNCTIONS_PER_BUS {
            return None;
        }
        Some(self.downstream[index].clone())
    }
}

pub struct PcieRoot {
    base: Arc<PcieUpstream>,
    inner: Arc<dyn PcieRootSwizzle>,
}

impl PcieRoot {
    pub fn swizzle(&self, dev_id: usize, func_id: usize, pin: usize) -> ZxResult<usize> {
        self.inner.swizzle(dev_id, func_id, pin)
    }
    pub fn managed_bus_id(&self) -> usize {
        self.base.managed_bus_id
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
    dev_lock: Mutex<()>,
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
        upstream: Weak<PcieUpstream>,
        dev_id: usize,
        func_id: usize,
    ) -> Option<Arc<Self>> {
        let ups = upstream.upgrade().unwrap();
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
            ..Self::default()
        });
        inst.init(upstream);
        Some(inst)
    }
    fn init(self: Arc<Self>, upstream: Weak<PcieUpstream>) -> ZxResult {
        let guard = self.dev_lock.lock();
        self.init_config(&guard)?;
        self.plugged_in = true;
        self.driver
            .upgrade()
            .unwrap()
            .link_device_to_upstream(Arc::downgrade(&self), upstream);
        Ok(())
    }
    fn init_config(&mut self, lock: &MutexGuard<()>) -> ZxResult {
        let cfg = self.cfg.as_ref().unwrap();
        self.vendor_id = cfg.read16(PciReg16::VendorId);
        self.device_id = cfg.read16(PciReg16::DeviceId);
        self.class_id = cfg.read8(PciReg8::BaseClass);
        self.subclass_id = cfg.read8(PciReg8::SubClass);
        self.prog_if = cfg.read8(PciReg8::ProgramInterface);
        self.rev_id = cfg.read8(PciReg8::RevisionId);
        self.init_probe_bars(lock);
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
