//! CPU information.

use crate::utils::init_once::InitOnce;

pub(super) static CPU_FREQ_MHZ: InitOnce<u16> = InitOnce::new_with_default(10);

hal_fn_impl! {
    impl mod crate::hal_fn::cpu {
        fn cpu_id() -> u8 {
            let mut cpu_id: i64;
            unsafe {
                asm!("mv {0}, tp", out(reg) cpu_id);
            }
            (cpu_id & 0xff) as u8
        }

        fn cpu_frequency() -> u16 {
            *CPU_FREQ_MHZ
        }
    }
}
