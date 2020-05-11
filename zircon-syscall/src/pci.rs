use super::*;
use zircon_object::dev::*;

impl Syscall<'_> {
    pub fn sys_pci_add_subtract_io_range(
        &self,
        handle: HandleValue,
        mmio: bool,
        base: u64,
        len: u64,
        add: bool,
    ) -> ZxResult {
        info!(
            "pci.add_subtract_io_range: handle_value={:#x}, mmio={:#}, base={:#x}, len={:#x}, add={:#}",
            handle, mmio, base, len, add
        );
        let proc = self.thread.proc();
        proc.get_object::<Resource>(handle)?
            .validate(ResourceKind::ROOT)?;
        let addr_space = if mmio {
            PciAddrSpace::MMIO
        } else {
            PciAddrSpace::PIO
        };
        if add {
            PCIeBusDriver::add_bus_region(base, len, addr_space)
        } else {
            PCIeBusDriver::sub_bus_region(base, len, addr_space)
        }
    }
    pub fn sys_pci_cfg_pio_rw(
        &self,
        handle: HandleValue,
        bus: u8,
        dev: u8,
        func: u8,
        offset: u8,
        mut val: UserInOutPtr<u32>,
        width: usize,
        write: bool,
    ) -> ZxResult {
        #[cfg(not(target_arch = "x86_64"))]
        return Err(ZxError::NOT_SUPPORTED);
        #[cfg(target_arch = "x86_64")]
        {
            info!(
                "pci.sys_pci_cfg_pio_rw: handle_value={:#x}, bus={:#x}, dev={:#x}, func={:#x}, offset={:#x}, width={:#x}, write={:#}",
                handle, bus, dev, func, offset, width, write
            );
            let proc = self.thread.proc();
            proc.get_object::<Resource>(handle)?
                .validate(ResourceKind::ROOT)?;
            if write {
                let tmp = val.read()?;
                pio_config_write(bus, dev, func, offset, tmp, width)?;
            } else {
                let mut tmp = 0;
                pio_config_read(bus, dev, func, offset, &mut tmp, width)?;
                val.write(tmp)?;
            }
            Ok(())
        }
    }
}
