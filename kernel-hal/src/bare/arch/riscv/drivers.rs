use alloc::{boxed::Box, sync::Arc};

use zcore_drivers::builder::{DeviceTreeDriverBuilder, IoMapper};
use zcore_drivers::irq::riscv::ScauseIntCode;
use zcore_drivers::scheme::IrqScheme;
use zcore_drivers::uart::BufferedUart;
use zcore_drivers::{Device, DeviceResult};

use crate::common::vm::GenericPageTable;
use crate::drivers::{IRQ, UART};
use crate::utils::init_once::InitOnce;
use crate::{mem::phys_to_virt, CachePolicy, MMUFlags, PhysAddr, VirtAddr};

static PLIC: InitOnce<Arc<dyn IrqScheme>> = InitOnce::new();

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

pub(super) fn init() -> DeviceResult {
    let dev_list =
        DeviceTreeDriverBuilder::new(phys_to_virt(crate::config::KCONFIG.dtb_paddr), IoMapperImpl)?
            .build()?;
    for dev in dev_list.into_iter() {
        match dev {
            Device::Irq(irq) => {
                if !IRQ.is_completed() {
                    IRQ.init_once_by(irq);
                } else {
                    PLIC.init_once_by(irq);
                }
            }
            Device::Uart(uart) => UART.init_once_by(BufferedUart::new(uart)),
            _ => {}
        }
    }

    IRQ.register_handler(
        ScauseIntCode::SupervisorSoft as _,
        Box::new(|| super::trap::super_soft()),
    )?;
    IRQ.register_handler(
        ScauseIntCode::SupervisorTimer as _,
        Box::new(|| super::trap::super_timer()),
    )?;
    IRQ.unmask(ScauseIntCode::SupervisorSoft as _)?;
    IRQ.unmask(ScauseIntCode::SupervisorTimer as _)?;

    Ok(())
}
