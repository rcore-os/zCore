pub mod interrupt;
mod io;
mod memory;
mod timer;

pub use interrupt::*;
pub use io::*;
pub use memory::*;
pub use timer::*;

use super::super::{Frame, PhysAddr};
use mips::paging::PageTable as MIPSPageTable;

pub struct Config {}

pub fn init(_config: Config) {
    intr_init();
    unsafe {
        set_page_table(0xFFFF_FFFF);
    }
    timer_init();
}

#[export_name = "hal_apic_local_id"]
pub fn apic_local_id() -> u8 {
    // unimplemented!()
    0
}

/// Page Table
#[repr(C)]
pub struct PageTableImpl {
    root_paddr: PhysAddr,
}

impl PageTableImpl {
    #[export_name = "hal_pt_current"]
    pub fn current() -> Self {
        PageTableImpl {
            root_paddr: get_page_table() & 0x7fffffff,
        }
    }

    /// Create a new `PageTable`.
    #[allow(clippy::new_without_default)]
    #[export_name = "hal_pt_new"]
    pub fn new() -> Self {
        let root_frame = Frame::alloc().expect("failed to alloc frame");

        let table = unsafe { &mut *(root_frame.paddr as *mut MIPSPageTable) };
        table.zero();
        trace!("create page table @ {:#x}", root_frame.paddr);
        PageTableImpl {
            root_paddr: root_frame.paddr,
        }
    }

    // fn get(&mut self) -> OffsetPageTable<'_> {
    //     // let root_vaddr = phys_to_virt(self.root_paddr);
    //     // let root = unsafe { &mut *(root_vaddr as *mut PageTable) };
    //     // let offset = x86_64::VirtAddr::new(phys_to_virt(0) as u64);
    //     // unsafe { OffsetPageTable::new(root, offset) }
    //     unimplemented!()
    // }
}
