use kernel_hal::{MMUFlags, PageTableTrait};
use rvm::{ArchRvmPageTable, GuestPhysAddr, HostPhysAddr, RvmPageTable, RvmPageTableFlags};

pub struct VmmPageTable {
    rvm_page_table: ArchRvmPageTable,
}

impl VmmPageTable {
    pub fn new() -> Self {
        Self {
            rvm_page_table: ArchRvmPageTable::new(),
        }
    }
}

impl PageTableTrait for VmmPageTable {
    fn map(
        &mut self,
        gpaddr: GuestPhysAddr,
        hpaddr: HostPhysAddr,
        flags: MMUFlags,
    ) -> Result<(), ()> {
        self.rvm_page_table
            .map(gpaddr, hpaddr, flags.to_rvm_flags())
            .map_err(|_| ())
    }

    fn unmap(&mut self, gpaddr: GuestPhysAddr) -> Result<(), ()> {
        self.rvm_page_table.unmap(gpaddr).map_err(|_| ())
    }

    fn protect(&mut self, gpaddr: GuestPhysAddr, flags: MMUFlags) -> Result<(), ()> {
        self.rvm_page_table
            .protect(gpaddr, flags.to_rvm_flags())
            .map_err(|_| ())
    }

    fn query(&mut self, gpaddr: GuestPhysAddr) -> Result<HostPhysAddr, ()> {
        self.rvm_page_table.query(gpaddr).map_err(|_| ())
    }

    fn table_phys(&self) -> HostPhysAddr {
        self.rvm_page_table.table_phys()
    }
}

trait FlagsExt {
    fn to_rvm_flags(self) -> RvmPageTableFlags;
}

impl FlagsExt for MMUFlags {
    fn to_rvm_flags(self) -> RvmPageTableFlags {
        let mut f = RvmPageTableFlags::empty();
        if self.contains(MMUFlags::READ) {
            f |= RvmPageTableFlags::READ;
        }
        if self.contains(MMUFlags::WRITE) {
            f |= RvmPageTableFlags::WRITE;
        }
        if self.contains(MMUFlags::EXECUTE) {
            f |= RvmPageTableFlags::EXECUTE;
        }
        f
    }
}
