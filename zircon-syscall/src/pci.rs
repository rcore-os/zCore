use super::*;
use zircon_object::dev::{PCIeBusDriver, PciAddrSpace, Resource, ResourceKind};

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
}
