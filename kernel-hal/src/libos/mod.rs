mod drivers;
mod dummy;
mod mock_mem;

pub mod boot;
pub mod config;
pub mod cpu;
pub mod interrupt;
pub mod mem;
pub mod net;
pub mod thread;
pub mod timer;
pub mod vdso;
pub mod vm;

#[path = "special.rs"]
#[doc(cfg(feature = "libos"))]
pub mod libos;

pub use super::hal_fn::rand;

hal_fn_impl_default!(rand, super::hal_fn::console);

#[cfg(target_os = "macos")]
mod macos;

/// Non-SMP initialization.
pub fn init() {
    drivers::init_early();
    boot::primary_init();
}
