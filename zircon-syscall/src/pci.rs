use super::*;
use alloc::vec::Vec;
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
    #[allow(unreachable_code, unused_variables, unused_mut)]
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
                let mut tmp = pio_config_read(bus, dev, func, offset, width)?;
                val.write(tmp)?;
            }
            Ok(())
        }
    }
    pub fn sys_pci_init(&self, handle: HandleValue, init_buf: usize, len: u32) -> ZxResult {
        info!(
            "pci.init: handle_value={:#x}, init_buf={:#x}, len={:#x}",
            handle, init_buf, len
        );
        let proc = self.thread.proc();
        proc.get_object::<Resource>(handle)?
            .validate(ResourceKind::ROOT)?;
        if len > PCI_INIT_ARG_MAX_SIZE as u32 {
            return Err(ZxError::INVALID_ARGS);
        }
        #[allow(non_snake_case)]
        let HEADER_SIZE = core::mem::size_of::<PciInitArgsHeader>();
        #[allow(non_snake_case)]
        let ADDR_WINDOWS_SIZE = core::mem::size_of::<PciInitArgsAddrWindows>();
        let arg_header_in: UserInPtr<PciInitArgsHeader> = init_buf.into();
        let mut arg_header = arg_header_in.read()?;
        let expected_len = HEADER_SIZE + arg_header.addr_window_count as usize * ADDR_WINDOWS_SIZE;
        if len != expected_len as u32 {
            return Err(ZxError::INVALID_ARGS);
        }
        let mut init_buf = init_buf + core::mem::size_of::<PciInitArgsHeader>();
        let mut addr_windows: Vec<PciInitArgsAddrWindows> = Vec::new();
        for _i in 0..arg_header.addr_window_count {
            let arg_win_in: UserInPtr<PciInitArgsAddrWindows> = init_buf.into();
            let arg_win = arg_win_in.read()?;
            addr_windows.push(arg_win);
            init_buf += ADDR_WINDOWS_SIZE;
        }
        // Configure interrupts
        pci_configure_interrupt(&mut arg_header)?;
        if arg_header.addr_window_count != 1 {
            return Err(ZxError::INVALID_ARGS); // for non DesignWare Controller
        }
        let addr_win = &mut addr_windows[0];
        if addr_win.bus_start != 0 || addr_win.bus_start > addr_win.bus_end {
            return Err(ZxError::INVALID_ARGS);
        }
        // Some systems will report overly large PCIe config regions
        // that collide with architectural registers.
        #[cfg(target_arch = "x86_64")]
        {
            let num_buses: u8 = addr_win.bus_end - addr_win.bus_start + 1;
            let mut end: u64 = addr_win.base + num_buses as u64 * PCIE_ECAM_BYTES_PER_BUS as u64;
            let high_limit: u64 = 0xfec00000;
            if end > high_limit {
                end = high_limit;
                if end < addr_win.base {
                    return Err(ZxError::INVALID_ARGS);
                }
                addr_win.size =
                    ((end - addr_win.base) & (PCIE_ECAM_BYTES_PER_BUS as u64 - 1)) as usize;
                let new_bus_end: usize =
                    addr_win.size / PCIE_ECAM_BYTES_PER_BUS + addr_win.bus_start as usize - 1;
                if new_bus_end as usize >= PCIE_MAX_BUSSES {
                    return Err(ZxError::INVALID_ARGS);
                }
                addr_win.bus_end = new_bus_end as u8;
            }
        }
        if addr_win.cfg_space_type == PCI_CFG_SPACE_TYPE_MMIO {
            if addr_win.size < PCIE_ECAM_BYTES_PER_BUS
                || addr_win.size / PCIE_ECAM_BYTES_PER_BUS
                    > PCIE_MAX_BUSSES - addr_win.bus_start as usize
            {
                return Err(ZxError::INVALID_ARGS);
            }
            let addr_provider = Arc::new(MmioPcieAddressProvider::new());
            addr_provider.add_ecam(PciEcamRegion {
                phys_base: addr_win.base,
                size: addr_win.size,
                bus_start: addr_win.bus_start,
                bus_end: addr_win.bus_end,
            })?;
            PCIeBusDriver::set_address_translation_provider(addr_provider)?;
        } else if addr_win.cfg_space_type == PCI_CFG_SPACE_TYPE_PIO {
            let addr_provider = Arc::new(PioPcieAddressProvider::new());
            PCIeBusDriver::set_address_translation_provider(addr_provider)?;
        } else {
            return Err(ZxError::INVALID_ARGS);
        }
        let root = PcieRootLUTSwizzle::new(pcie, 0, arg_header.dev_pin_to_global_irq);
        PCIeBusDriver::add_root(root)?;
        PCIeBusDriver::start_bus_driver()?;
        Ok(())
    }
}
