use crate::{PhysAddr, VirtAddr};
use cortex_a::registers::*;
use tock_registers::interfaces::{Writeable, Readable};

hal_fn_impl! {
    impl mod crate::hal_fn::vm {
        fn activate_paging(vmtoken: PhysAddr) {
            info!("set page_table @ {:#x}", vmtoken);
            TTBR1_EL1.set(vmtoken as _);
        }

        fn current_vmtoken() -> PhysAddr {
            TTBR1_EL1.get() as _
        }

        fn flush_tlb(vaddr: Option<VirtAddr>) {
            // Translations used at EL1 for the specified address, for all ASID values,
            // in the Inner Shareable shareability domain.
            unsafe {
                core::arch::asm!(
                    "dsb ishst
                    tlbi vaae1is, {0}
                    dsb ish
                    isb",
                    in(reg) vaddr.unwrap() >> 12
                );
            }
        }
    }
}