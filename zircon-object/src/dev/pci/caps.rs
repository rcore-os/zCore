use super::config::PciConfig;
use super::nodes::PcieDeviceType;
use core::convert::TryFrom;

pub enum PciCapacity {
    Msi(PciCapacityStd, PciCapacityMsi),
    Pcie(PciCapacityStd, PciCapPcie),
    AdvFeatures(PciCapacityStd, PciCapAdvFeatures),
    Std(PciCapacityStd),
}

pub struct PciCapacityStd {
    pub id: u8,
    pub base: u16,
}

impl PciCapacityStd {
    pub fn create(base: u16, id: u8) -> PciCapacityStd {
        PciCapacityStd { id, base }
    }
}

#[derive(Default)]
pub struct PciMsiBlock {
    pub target_addr: u64,
    pub allocated: bool,
    pub base_irq: u32,
    pub num_irq: u32,
    pub target_data: u32,
}

pub struct PciCapacityMsi {
    pub msi_size: u16,
    pub has_pvm: bool,
    pub is_64bit: bool,
    pub max_irq: u32,
    pub irq_block: PciMsiBlock,
}

impl PciCapacityMsi {
    pub fn create(cfg: &PciConfig, base: u16, id: u8) -> PciCapacityMsi {
        assert_eq!(id, 0x5); // PCIE_CAP_ID_MSI
        let ctrl = cfg.read16_offset(base as usize + 0x2);
        let has_pvm = (ctrl & 0x100) != 0;
        let is_64bit = (ctrl & 0x80) != 0;
        cfg.write16_offset(base as usize + 0x2, ctrl & !0x71);
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
            irq_block: PciMsiBlock::default(),
        }
    }
}

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
