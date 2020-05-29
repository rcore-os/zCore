use super::super::{ZxError, ZxResult};
use super::config::PciConfig;
use super::nodes::PcieDeviceType;
use alloc::boxed::Box;
use core::convert::TryFrom;
use kernel_hal::InterruptManager;
use spin::*;

#[derive(Debug)]
pub enum PciCapacity {
    Msi(PciCapacityStd, PciCapacityMsi),
    Pcie(PciCapacityStd, PciCapPcie),
    AdvFeatures(PciCapacityStd, PciCapAdvFeatures),
    Std(PciCapacityStd),
}

#[derive(Debug)]
pub struct PciCapacityStd {
    pub id: u8,
    pub base: u16,
}

impl PciCapacityStd {
    pub fn create(base: u16, id: u8) -> PciCapacityStd {
        PciCapacityStd { id, base }
    }
    pub fn is_valid(&self) -> bool {
        true
    }
}

#[derive(Default, Clone, Copy)]
pub struct PciMsiBlock {
    pub target_addr: u64,
    pub allocated: bool,
    pub base_irq: u32,
    pub num_irq: u32,
    pub target_data: u32,
}

impl PciMsiBlock {
    pub fn allocate_msi_block(irq_num: u32) -> ZxResult<Self> {
        if irq_num == 0 || irq_num > 32 {
            return Err(ZxError::INVALID_ARGS);
        }
        if let Some((start, size)) = InterruptManager::allocate_block(irq_num) {
            Ok(PciMsiBlock {
                target_addr: (0xFEE00000 | 0x08) & !0x4,
                target_data: start as u32,
                base_irq: start as u32,
                num_irq: size as u32,
                allocated: true,
            })
        } else {
            Err(ZxError::NO_RESOURCES)
        }
    }
    pub fn free_msi_block(&self) {
        InterruptManager::free_block(self.base_irq, self.num_irq)
    }
    pub fn register_handler(&self, msi_id: u32, handle: Box<dyn Fn() + Send + Sync>) {
        assert!(self.allocated);
        assert!(msi_id < self.num_irq);
        InterruptManager::overwrite_handler(self.base_irq + msi_id, handle);
    }
}

// @see PCI Local Bus Specification 3.0 Section 6.8.1
#[derive(Debug, Clone, Copy)]
pub struct PciCapacityMsi {
    pub msi_size: u16,
    pub has_pvm: bool,
    pub is_64bit: bool,
    pub max_irq: u32,
    pub irq_block: Mutex<PciMsiBlock>,
    pub addr_upper_offset: usize,   // reg32
    pub data_offset: usize,         // reg16
    pub mask_bits_offset: usize,    // reg32
    pub pending_bits_offset: usize, // reg32
}

impl PciCapacityMsi {
    pub fn create(cfg: &PciConfig, base: usize, id: u8) -> PciCapacityMsi {
        assert_eq!(id, 0x5); // PCIE_CAP_ID_MSI
        let ctrl = cfg.read16_offset(base + 0x2);
        let has_pvm = (ctrl & 0x100) != 0;
        let is_64bit = (ctrl & 0x80) != 0;
        cfg.write16_offset(base as usize + 0x2, ctrl & !0x71);
        let mask_bits = Self::mask_bits_offset(is_64bit) + base as usize;
        if has_pvm {
            cfg.write32_offset(mask_bits, 0xffffffff);
        }
        PciCapacityMsi {
            msi_size: if has_pvm {
                if is_64bit {
                    20
                } else {
                    16
                }
            } else {
                if is_64bit {
                    14
                } else {
                    10
                }
            },
            has_pvm,
            is_64bit,
            max_irq: 0x1 << ((ctrl >> 1) & 0x7),
            irq_block: Mutex::new(PciMsiBlock::default()),
            addr_upper_offset: if is_64bit {
                base + 0x8
            } else {
                0 /*shouldn't use it*/
            },
            data_offset: base + if is_64bit { 0xC } else { 0x8 },
            mask_bits_offset: base + if is_64bit { 0x10 } else { 0xC },
            pending_bits_offset: base + if is_64bit { 0x14 } else { 0x10 },
        }
    }
    pub fn ctrl_offset() -> usize {
        0x2
    }
    pub fn mask_bits_offset(is_64bit: bool) -> usize {
        if is_64bit {
            0x10
        } else {
            0x0c
        }
    }
    pub fn addr_offset(is_64bit: bool) -> usize {
        if is_64bit {
            0xC
        } else {
            0x8
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PciCapPcie {
    pub version: u8,
    pub dev_type: PcieDeviceType,
    pub has_flr: bool,
}

impl PciCapPcie {
    pub fn create(cfg: &PciConfig, base: u16, id: u8) -> PciCapPcie {
        assert_eq!(id, 0x10); // PCIE_CAP_ID_PCI_EXPRESS
        let caps = cfg.read8_offset(base as usize + 0x2);
        let device_caps = cfg.read32_offset(base as usize + 0x4);
        PciCapPcie {
            version: ((caps >> 0) & 0xF) as u8,
            dev_type: PcieDeviceType::try_from(((caps >> 4) & 0xF) as u8).unwrap(),
            has_flr: ((device_caps >> 28) & 0x1) != 0,
        }
    }
}

#[derive(Debug)]
pub struct PciCapAdvFeatures {
    pub has_flr: bool,
    pub has_tp: bool,
}

impl PciCapAdvFeatures {
    pub fn create(cfg: &PciConfig, base: u16, id: u8) -> PciCapAdvFeatures {
        assert_eq!(id, 0x13); // PCIE_CAP_ID_ADVANCED_FEATURES
        let caps = cfg.read8_offset(base as usize + 0x3);
        PciCapAdvFeatures {
            has_flr: ((caps >> 1) & 0x1) != 0,
            has_tp: ((caps >> 0) & 0x1) != 0,
        }
    }
}
