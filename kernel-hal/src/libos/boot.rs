use crate::{KernelConfig, KernelHandler, KCONFIG, KHANDLER};

hal_fn_impl! {
    impl mod crate::hal_fn::boot {
        fn cmdline() -> String {
            let root_proc = std::env::args().skip(1).collect::<Vec<_>>().join("?");
            let mut cmdline = format!("ROOTPROC={}", root_proc);
            if let Ok(level) = std::env::var("LOG") {
                cmdline += &format!(":LOG={}", level);
            }
            cmdline
        }

        fn primary_init_early(cfg: KernelConfig, handler: &'static impl KernelHandler) {
            KCONFIG.init_once_by(cfg);
            KHANDLER.init_once_by(handler);
            super::drivers::init_early();
        }

        fn primary_init() {
            super::drivers::init();

            #[cfg(target_os = "macos")]
            unsafe {
                super::macos::register_sigsegv_handler();
            }
        }
    }
}
