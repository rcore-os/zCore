mod drivers;
mod dummy;
mod mem_common;

pub mod config;
pub mod mem;
pub mod thread;
pub mod timer;
pub mod vdso;
pub mod vm;

#[path = "special.rs"]
pub mod libos;

pub use super::hal_fn::{context, cpu, interrupt, rand};

hal_fn_impl_default!(context, cpu, interrupt, rand, super::hal_fn::serial);

cfg_if! {
    if #[cfg(target_os = "linux")] {
        pub mod dev;
    } else {
        pub use super::hal_fn::dev;
        hal_fn_impl_default!(dev::fb, dev::input);
    }
}

#[cfg(target_os = "macos")]
include!("macos.rs");

/// Initialize the HAL.
///
/// This function must be called at the beginning.
pub fn init() {
    let _ = crate::KCONFIG;
    crate::KHANDLER.init_once_by(&crate::kernel_handler::DummyKernelHandler);

    drivers::init();

    #[cfg(target_os = "macos")]
    unsafe {
        register_sigsegv_handler();
    }
}
