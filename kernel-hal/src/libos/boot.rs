hal_fn_impl! {
    impl mod crate::hal_fn::boot {
        fn primary_init() {
            let _ = crate::KCONFIG;
            crate::KHANDLER.init_once_by(&crate::kernel_handler::DummyKernelHandler);

            super::drivers::init();

            #[cfg(target_os = "macos")]
            unsafe {
                super::macos::register_sigsegv_handler();
            }
        }
    }
}
