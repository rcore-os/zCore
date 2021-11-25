//! CPU information.

hal_fn_impl! {
    impl mod crate::hal_fn::cpu {
        fn cpu_frequency() -> u16 {
            const DEFAULT: u16 = 2600;
            DEFAULT
        }

        fn cpu_id() -> u8 {
            let mut cpu_id;
            unsafe {
                asm!("mv {0}, tp", out(reg) cpu_id);
            }
            cpu_id
        }
    }
}
