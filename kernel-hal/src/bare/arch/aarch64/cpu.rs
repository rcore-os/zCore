//! CPU information.

use cortex_a::registers::*;
use tock_registers::interfaces::Readable;

hal_fn_impl! {
    impl mod crate::hal_fn::cpu {
        fn cpu_id() -> u8 {
            let id = MPIDR_EL1.get() & 0x3;
            id as u8
        }

        fn cpu_frequency() -> u16 {
            0
        }

        fn reset() -> ! {
            info!("shutdown...");
            let psci_system_off = 0x8400_0008_usize;
            unsafe {
                core::arch::asm!(
                    "hvc #0",
                    in("x0") psci_system_off
                );
            }
            unreachable!()
        }
    }
}
