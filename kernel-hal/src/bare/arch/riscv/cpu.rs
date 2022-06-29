//! CPU information.
use crate::utils::init_once::InitOnce;

cfg_if::cfg_if! {
    if #[cfg(feature = "board-qemu")] {
        pub(super) static CPU_FREQ_MHZ: InitOnce<u16> = InitOnce::new_with_default(12); // 12.5MHz
    } else {
        pub(super) static CPU_FREQ_MHZ: InitOnce<u16> = InitOnce::new_with_default(1000); // 1GHz
    }
}

hal_fn_impl! {
    impl mod crate::hal_fn::cpu {
        fn cpu_id() -> u8 {
            let mut cpu_id;
            unsafe { core::arch::asm!("mv {0}, tp", out(reg) cpu_id) };
            cpu_id
        }

        fn cpu_frequency() -> u16 {
            *CPU_FREQ_MHZ
        }

        fn reset() -> ! {
            info!("shutdown...");
            sbi_rt::system_reset(sbi_rt::RESET_TYPE_SHUTDOWN, sbi_rt::RESET_REASON_NO_REASON);
            unreachable!()
        }
    }
}
