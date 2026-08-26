//! CPU information.

hal_fn_impl! {
    impl mod crate::hal_fn::cpu {
        fn cpu_id() -> u8 {
            std::thread::current().id().as_u64().get() as u8
        }

        fn cpu_brand() -> alloc::string::String {
            alloc::string::String::from("Host CPU")
        }

        fn cpu_count() -> u8 {
            std::thread::available_parallelism()
                .map(|n| n.get() as u8)
                .unwrap_or(1)
        }

        fn reset() -> ! {
            info!("shutdown...");
            std::process::exit(0);
        }
    }
}
