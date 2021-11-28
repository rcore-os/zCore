mod drivers;
pub mod sbi;
mod trap;

pub mod config;
pub mod cpu;
pub mod interrupt;
pub mod mem;
pub mod timer;
pub mod vm;

use alloc::{string::String, vec::Vec};
use core::ops::Range;
use zcore_drivers::utils::devicetree::Devicetree;
use zcore_drivers::irq::riscv::ScauseIntCode;

use crate::{mem::phys_to_virt, utils::init_once::InitOnce, PhysAddr};

static CMDLINE: InitOnce<String> = InitOnce::new_with_default(String::new());
static INITRD_REGION: InitOnce<Option<Range<PhysAddr>>> = InitOnce::new_with_default(None);
static MEMORY_REGIONS: InitOnce<Vec<Range<PhysAddr>>> = InitOnce::new_with_default(Vec::new());

pub fn cmdline() -> String {
    CMDLINE.clone()
}

pub fn init_ram_disk() -> Option<&'static mut [u8]> {
    INITRD_REGION.as_ref().map(|range| unsafe {
        core::slice::from_raw_parts_mut(phys_to_virt(range.start) as *mut u8, range.len())
    })
}

pub fn primary_init_early() {
    let dt = Devicetree::from(phys_to_virt(crate::KCONFIG.dtb_paddr)).unwrap();
    if let Some(cmdline) = dt.bootargs() {
        CMDLINE.init_once_by(cmdline.into());
    }
    if let Some(initrd_region) = dt.initrd_region() {
        INITRD_REGION.init_once_by(Some(initrd_region));
    }
    if let Ok(regions) = dt.memory_regions() {
        MEMORY_REGIONS.init_once_by(regions);
    }
}

pub fn primary_init() {
    vm::init();
    drivers::init().unwrap();
    timer::init();
}

pub fn secondary_init() {
    log::warn!("secondary init");
    vm::init();
    let irq = crate::drivers::all_irq()
        .find("riscv-plic")
        .expect("IRQ device 'riscv-plic' not initialized!");
    irq.unmask(ScauseIntCode::SupervisorSoft as usize).unwrap();
    irq.unmask(ScauseIntCode::SupervisorTimer as usize).unwrap();
    irq.init_hart();
}
