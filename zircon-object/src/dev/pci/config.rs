use super::pio::{pio_config_read_addr, pio_config_write_addr};
use super::PciAddrSpace;
use numeric_enum_macro::numeric_enum;

#[derive(Debug)]
pub struct PciConfig {
    pub addr_space: PciAddrSpace,
    pub base: usize,
}

#[allow(unsafe_code)]
impl PciConfig {
    pub fn read8_offset(&self, offset: usize) -> u8 {
        trace!("read8 @ {:#x?}", offset);
        match self.addr_space {
            PciAddrSpace::MMIO => unsafe { u8::from_le(*(offset as *const u8)) },
            PciAddrSpace::PIO => pio_config_read_addr(offset as u32, 8).unwrap() as u8,
        }
    }
    pub fn read16_offset(&self, addr: usize) -> u16 {
        trace!("read16 @ {:#x?}", addr);
        match self.addr_space {
            PciAddrSpace::MMIO => unsafe { u16::from_le(*(addr as *const u16)) },
            PciAddrSpace::PIO => pio_config_read_addr(addr as u32, 16).unwrap() as u16,
        }
    }
    pub fn read32_offset(&self, addr: usize) -> u32 {
        trace!("read32 @ {:#x?}", addr);
        match self.addr_space {
            PciAddrSpace::MMIO => unsafe { u32::from_le(*(addr as *const u32)) },
            PciAddrSpace::PIO => pio_config_read_addr(addr as u32, 32).unwrap(),
        }
    }
    pub fn read8(&self, addr: PciReg8) -> u8 {
        self.read8_offset(self.base + addr as usize)
    }
    pub fn read8_(&self, addr: usize) -> u8 {
        self.read8_offset(self.base + addr)
    }
    pub fn read16(&self, addr: PciReg16) -> u16 {
        self.read16_offset(self.base + addr as usize)
    }
    pub fn read16_(&self, addr: usize) -> u16 {
        self.read16_offset(self.base + addr)
    }
    pub fn read32(&self, addr: PciReg32) -> u32 {
        self.read32_offset(self.base + addr as usize)
    }
    pub fn read32_(&self, addr: usize) -> u32 {
        self.read32_offset(self.base + addr)
    }
    pub fn read_bar(&self, bar_: usize) -> u32 {
        self.read32_offset(self.base + PciReg32::BARBase as usize + bar_ * 4)
    }

    pub fn write8_offset(&self, addr: usize, val: u8) {
        match self.addr_space {
            PciAddrSpace::MMIO => unsafe { *(addr as *mut u8) = val },
            PciAddrSpace::PIO => pio_config_write_addr(addr as u32, val as u32, 8).unwrap(),
        }
    }
    pub fn write16_offset(&self, addr: usize, val: u16) {
        trace!(
            "write16 @ {:#x?}, addr_space = {:#x?}",
            addr,
            self.addr_space
        );
        match self.addr_space {
            PciAddrSpace::MMIO => unsafe { *(addr as *mut u16) = val },
            PciAddrSpace::PIO => pio_config_write_addr(addr as u32, val as u32, 16).unwrap(),
        }
    }
    pub fn write32_offset(&self, addr: usize, val: u32) {
        match self.addr_space {
            PciAddrSpace::MMIO => unsafe { *(addr as *mut u32) = val },
            PciAddrSpace::PIO => pio_config_write_addr(addr as u32, val as u32, 32).unwrap(),
        }
    }
    pub fn write8(&self, addr: PciReg8, val: u8) {
        self.write8_offset(self.base + addr as usize, val)
    }
    pub fn write16(&self, addr: PciReg16, val: u16) {
        self.write16_offset(self.base + addr as usize, val)
    }
    pub fn write16_(&self, addr: usize, val: u16) {
        self.write16_offset(self.base + addr, val)
    }
    pub fn write32(&self, addr: PciReg32, val: u32) {
        self.write32_offset(self.base + addr as usize, val)
    }
    pub fn write32_(&self, addr: usize, val: u32) {
        self.write32_offset(self.base + addr, val)
    }
    pub fn write_bar(&self, bar_: usize, val: u32) {
        self.write32_offset(self.base + PciReg32::BARBase as usize + bar_ * 4, val)
    }
}

numeric_enum! {
    #[repr(usize)]
    pub enum PciReg8 {
        // standard
        RevisionId = 0x8,
        ProgramInterface = 0x9,
        SubClass = 0xA,
        BaseClass = 0xB,
        CacheLineSize = 0xC,
        LatencyTimer = 0xD,
        HeaderType = 0xE,
        Bist = 0xF,

        // bridge
        PrimaryBusId = 0x18,
        SecondaryBusId = 0x19,
        SubordinateBusId = 0x1A,
        SecondaryLatencyTimer = 0x1B,
        IoBase = 0x1C,
        IoLimit = 0x1D,
        CapabilitiesPtr = 0x34,
        InterruptLine = 0x3C,
        InterruptPin = 0x3D,
        MinGrant = 0x3E,
        MaxLatency = 0x3F,
    }
}
numeric_enum! {
    #[repr(usize)]
    pub enum PciReg16 {
        // standard
        VendorId = 0x0,
        DeviceId = 0x2,
        Command = 0x4,
        Status = 0x6,

        // bridge
        SecondaryStatus = 0x1E,
        MemoryBase = 0x20,
        MemoryLimit = 0x22,
        PrefetchableMemoryBase = 0x24,
        PrefetchableMemoryLimit = 0x26,
        IoBaseUpper = 0x30,
        IoLimitUpper = 0x32,
        BridgeControl = 0x3E,
    }
}
numeric_enum! {
    #[repr(usize)]
    pub enum PciReg32 {
        // standard
        BARBase = 0x10,

        // bridge
        PrefetchableMemoryBaseUpper = 0x28,
        PrefetchableMemoryLimitUpper = 0x2C,
        BridgeExpansionRomAddress = 0x38,
    }
}

pub const PCIE_BASE_CONFIG_SIZE: usize = 256;
pub const PCIE_EXTENDED_CONFIG_SIZE: usize = 4096;
