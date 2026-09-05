//! Bootstrap and initialization.

use crate::{KernelConfig, KernelHandler, KCONFIG, KHANDLER};

hal_fn_impl! {
    impl mod crate::hal_fn::boot {
        fn primary_init_early(cfg: KernelConfig, handler: &'static impl KernelHandler) {
            KCONFIG.init_once_by(cfg);
            KHANDLER.init_once_by(handler);
            super::drivers::init_early();
        }

        fn primary_init() {
            super::drivers::init();

            #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
            unsafe {
                super::macos::register_sigsegv_handler();
            }

        }
    }
}
