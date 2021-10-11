mod drivers;
mod dummy;
mod mock_mem;

pub mod config;
pub mod mem;
pub mod thread;
pub mod timer;
pub mod vdso;
pub mod vm;

#[path = "special.rs"]
pub mod libos;

pub use super::hal_fn::{context, cpu, interrupt, rand};

hal_fn_impl_default!(context, cpu, interrupt, rand, super::hal_fn::console);

#[cfg(target_os = "macos")]
mod macos;

/// Initialize the HAL.
///
/// This function must be called at the beginning.
pub fn init() {
    let _ = crate::KCONFIG;
    crate::KHANDLER.init_once_by(&crate::kernel_handler::DummyKernelHandler);

    drivers::init();

    #[cfg(target_os = "macos")]
    unsafe {
        macos::register_sigsegv_handler();
    }
}
