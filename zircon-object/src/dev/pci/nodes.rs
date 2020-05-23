use super::*;
use alloc::sync::*;
pub struct PcieUpstream {
    pub driver: Weak<PCIeBusDriver>,
    pub managed_bus_id: usize,
    downstream: [Arc<PciDevices>; PCI_MAX_FUNCTIONS_PER_BUS],
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
                let vender_id = cfg.read16(PciReg16::VendorId);
                let mut good_device = vender_id as usize != PCIE_INVALID_VENDOR_ID;
                if good_device {
                    let device_id = cfg.read16(PciReg16::DeviceId);
                    info!(
                        "Found device {:#x?}:{:#x?} at {:#x?}:{:#x?}.{:#x?}\n",
                        vender_id, device_id, self.managed_bus_id, dev_id, func_id
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
                            if let None = self.scan_device(cfg, dev_id, func_id) {
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
    fn get_downstream(&self, index: usize) -> Option<Arc<PciDevices>> {
        if index >= PCI_MAX_FUNCTIONS_PER_BUS {
            return None;
        }
        Some(self.downstream[index].clone())
    }
}

pub struct PcieRoot {
    base: PcieUpstream,
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

const PCI_HEADER_TYPE_MULTI_FN: u8 = 0x80;
