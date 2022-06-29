use super::{config::PciConfig, nodes::PcieDeviceType};
use crate::{ZxError, ZxResult};

use alloc::boxed::Box;
use core::convert::TryFrom;
use kernel_hal::interrupt;
use lock::Mutex;

/// Enumeration for PCI capabilities.
#[derive(Debug)]
pub enum PciCapability {
    /// MSI Interrupts.
    Msi(PciCapabilityStd, PciCapabilityMsi),
    /// PCI Express Capability.
    Pcie(PciCapabilityStd, PciCapPcie),
    AdvFeatures(PciCapabilityStd, PciCapAdvFeatures),
    Std(PciCapabilityStd),
}

#[derive(Debug)]
pub struct PciCapabilityStd {
    pub id: u8,
    pub base: u16,
}

impl PciCapabilityStd {
    pub fn create(base: u16, id: u8) -> PciCapabilityStd {
        PciCapabilityStd { id, base }
    }
    pub fn is_valid(&self) -> bool {
        true
    }
}

#[derive(Default, Clone, Copy, Debug)]
pub struct PciMsiBlock {
    pub target_addr: u64,
    pub allocated: bool,
    pub base_irq: usize,
    pub num_irq: usize,
    pub target_data: u32,
}

impl PciMsiBlock {
    pub fn allocate(irq_num: usize) -> ZxResult<Self> {
        if irq_num == 0 || irq_num > 32 {
            return Err(ZxError::INVALID_ARGS);
        }
        let range = interrupt::msi_alloc_block(irq_num).map_err(|_| ZxError::NO_RESOURCES)?;
        Ok(PciMsiBlock {
            target_addr: (0xFEE0_0000 | 0x08) & !0x4,
            target_data: range.start as u32,
            base_irq: range.start,
            num_irq: range.len(),
            allocated: true,
        })
    }
    pub fn free(&self) {
        interrupt::msi_free_block(self.base_irq..self.base_irq + self.num_irq).ok();
    }
    pub fn register_handler(&self, msi_id: usize, handle: Box<dyn Fn() + Send + Sync>) {
        assert!(self.allocated);
        interrupt::msi_register_handler(
            self.base_irq..self.base_irq + self.num_irq,
            msi_id,
            handle,
        )
        .unwrap();
    }
}

// @see PCI Local Bus Specification 3.0 Section 6.8.1
#[derive(Debug)]
pub struct PciCapabilityMsi {
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

impl PciCapabilityMsi {
    pub fn create(cfg: &PciConfig, base: usize, id: u8) -> PciCapabilityMsi {
        assert_eq!(id, 0x5); // PCIE_CAP_ID_MSI
        let ctrl = cfg.read16_(base + 0x2);
        let has_pvm = (ctrl & 0x100) != 0;
        let is_64bit = (ctrl & 0x80) != 0;
        cfg.write16_(base + 0x2, ctrl & !0x71);
        let mask_bits = base + if is_64bit { 0x10 } else { 0xC };
        if has_pvm {
            cfg.write32_offset(mask_bits, 0xffff_ffff);
        }
        PciCapabilityMsi {
            msi_size: match (has_pvm, is_64bit) {
                (true, true) => 20,
                (true, false) => 16,
                (false, true) => 14,
                (false, false) => 10,
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

/// PCI Express Capability.
#[derive(Debug, Clone, Copy)]
pub struct PciCapPcie {
    pub version: u8,
    pub dev_type: PcieDeviceType,
    pub has_flr: bool,
}

impl PciCapPcie {
    pub fn create(cfg: &PciConfig, base: u16, id: u8) -> PciCapPcie {
        assert_eq!(id, 0x10); // PCIE_CAP_ID_PCI_EXPRESS
        let caps = cfg.read8_(base as usize + 0x2);
        let device_caps = cfg.read32_(base as usize + 0x4);
        PciCapPcie {
            version: caps & 0xF,
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
        let caps = cfg.read8_(base as usize + 0x3);
        PciCapAdvFeatures {
            has_flr: ((caps >> 1) & 0x1) != 0,
            has_tp: (caps & 0x1) != 0,
        }
    }
}
