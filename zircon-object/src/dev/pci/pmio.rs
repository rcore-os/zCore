#![allow(missing_docs)]

use crate::{ZxError, ZxResult};

/// Returns the BDF address without the bottom two bits masked off.
pub fn pci_bdf_raw_addr(bus: u8, dev: u8, func: u8, offset: u8) -> u32 {
    ((bus as u32 & 0xff) << 16)         // bits 23-16 bus
        | ((dev as u32 & 0x1f) << 11)   // bits 15-11 device
        | ((func as u32 & 0x7) << 8)    // bits 10-8 func
        | (offset as u32 & 0xff) // bits 7-2 reg, with bottom 2 bits as well
}

cfg_if::cfg_if! {
if #[cfg(all(target_arch = "x86_64", target_os = "none"))] {
    use kernel_hal::x86_64::{Io, Pmio};
    use lock::Mutex;

    static PIO_LOCK: Mutex<()> = Mutex::new(());
    const PCI_CONFIG_ADDR: u16 = 0xcf8;
    const PCI_CONFIG_DATA: u16 = 0xcfc;
    const PCI_CONFIG_ENABLE: u32 = 1 << 31;

    pub fn pmio_config_read_addr(addr: u32, width: usize) -> ZxResult<u32> {
        let mut port_cfg = Pmio::<u32>::new(PCI_CONFIG_ADDR);
        let port_data = Pmio::<u32>::new(PCI_CONFIG_DATA);

        let _lock = PIO_LOCK.lock();
        let shift = ((addr & 0x3) << 3) as usize;
        if shift + width > 32 {
            return Err(ZxError::INVALID_ARGS);
        }
        port_cfg.write((addr & !0x3) | PCI_CONFIG_ENABLE);
        let tmp_val = u32::from_le(port_data.read());
        Ok((tmp_val >> shift) & (((1u64 << width) - 1) as u32))
    }
    pub fn pmio_config_write_addr(addr: u32, val: u32, width: usize) -> ZxResult {
        let mut port_cfg = Pmio::<u32>::new(PCI_CONFIG_ADDR);
        let mut port_data = Pmio::<u32>::new(PCI_CONFIG_DATA);

        let _lock = PIO_LOCK.lock();
        let shift = ((addr & 0x3) << 3) as usize;
        if shift + width > 32 {
            return Err(ZxError::INVALID_ARGS);
        }
        port_cfg.write((addr & !0x3) | PCI_CONFIG_ENABLE);
        let width_mask = ((1u64 << width) - 1) as u32;
        let val = val & width_mask;
        let tmp_val = if width < 32 {
            (u32::from_le(port_data.read()) & !(width_mask << shift)) | (val << shift)
        } else {
            val
        };
        port_data.write(u32::to_le(tmp_val));
        Ok(())
    }
} else {
    pub fn pmio_config_read_addr(_addr: u32, _width: usize) -> ZxResult<u32> {
        Err(ZxError::NOT_SUPPORTED)
    }
    pub fn pmio_config_write_addr(_addr: u32, _val: u32, _width: usize) -> ZxResult {
        Err(ZxError::NOT_SUPPORTED)
    }
}
} // cfg_if!

pub fn pio_config_read(bus: u8, dev: u8, func: u8, offset: u8, width: usize) -> ZxResult<u32> {
    pmio_config_read_addr(pci_bdf_raw_addr(bus, dev, func, offset), width)
}

pub fn pio_config_write(
    bus: u8,
    dev: u8,
    func: u8,
    offset: u8,
    val: u32,
    width: usize,
) -> ZxResult {
    pmio_config_write_addr(pci_bdf_raw_addr(bus, dev, func, offset), val, width)
}
