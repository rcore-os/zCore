use alloc::boxed::Box;
use alloc::format;

use zcore_drivers::builder::{DevicetreeDriverBuilder, IoMapper};
use zcore_drivers::irq::riscv::ScauseIntCode;
use zcore_drivers::uart::BufferedUart;
use zcore_drivers::{Device, DeviceResult};

use crate::common::vm::GenericPageTable;
use crate::{drivers, mem::phys_to_virt, CachePolicy, MMUFlags, PhysAddr, VirtAddr};

struct IoMapperImpl;

impl IoMapper for IoMapperImpl {
    fn query_or_map(&self, paddr: PhysAddr, size: usize) -> Option<VirtAddr> {
        let vaddr = phys_to_virt(paddr);
        let mut pt = super::vm::kernel_page_table().lock();
        if let Ok((paddr_mapped, _, _)) = pt.query(vaddr) {
            if paddr_mapped == paddr {
                Some(vaddr)
            } else {
                warn!(
                    "IoMapper::query_or_map: not linear mapping: vaddr={:#x}, paddr={:#x}",
                    vaddr, paddr_mapped
                );
                None
            }
        } else {
            let size = crate::addr::align_up(size);
            let flags = MMUFlags::READ
                | MMUFlags::WRITE
                | MMUFlags::HUGE_PAGE
                | MMUFlags::from_bits_truncate(CachePolicy::UncachedDevice as usize);
            if let Err(err) = pt.map_cont(vaddr, size, paddr, flags) {
                warn!(
                    "IoMapper::query_or_map: failed to map {:#x?} => {:#x}, flags={:?}: {:?}",
                    vaddr..vaddr + size,
                    paddr,
                    flags,
                    err
                );
                None
            } else {
                Some(vaddr)
            }
        }
    }
}

/// Initialize device drivers.
pub(super) fn init() -> DeviceResult {
    // prase DTB and probe devices
    let dev_list =
        DevicetreeDriverBuilder::new(phys_to_virt(crate::KCONFIG.dtb_paddr), IoMapperImpl)?
            .build()?;
    // add drivers
    for dev in dev_list.into_iter() {
        if let Device::Uart(uart) = dev {
            drivers::add_device(Device::Uart(BufferedUart::new(uart)));
        } else {
            drivers::add_device(dev);
        }
    }

    #[cfg(not(any(feature = "loopback", feature = "board-d1")))]
    {
        use alloc::sync::Arc;
        use zcore_drivers::bus::pci;
        let pci_devs = pci::init(Some(Arc::new(IoMapperImpl)))?;
        for d in pci_devs.into_iter() {
            drivers::add_device(d);
        }
    }

    intc_init()?;

    #[cfg(feature = "graphic")]
    if let Some(display) = drivers::all_display().first() {
        crate::console::init_graphic_console(display.clone());
        if display.need_flush() {
            // TODO: support nested interrupt to render in time
            crate::thread::spawn(crate::common::future::DisplayFlushFuture::new(display, 30));
        }
    }

    #[cfg(feature = "loopback")]
    {
        use crate::net;
        net::init();
    }

    Ok(())
}

pub(super) fn intc_init() -> DeviceResult {
    let irq = drivers::all_irq()
        .find(format!("riscv-intc-cpu{}", crate::cpu::cpu_id()).as_str())
        .expect("IRQ device 'riscv-intc' not initialized!");
    // register soft interrupts handler
    irq.register_handler(
        ScauseIntCode::SupervisorSoft as _,
        Box::new(super::trap::super_soft),
    )?;
    // register timer interrupts handler
    irq.register_handler(
        ScauseIntCode::SupervisorTimer as _,
        Box::new(super::trap::super_timer),
    )?;
    irq.unmask(ScauseIntCode::SupervisorSoft as _)?;
    irq.unmask(ScauseIntCode::SupervisorTimer as _)?;

    Ok(())
}
