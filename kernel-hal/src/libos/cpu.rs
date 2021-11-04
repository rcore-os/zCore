//! CPU information.

hal_fn_impl! {
    impl mod crate::hal_fn::cpu {
        fn cpu_id() -> u8 {
            std::thread::current().id().as_u64().get() as u8
        }
    }
}
