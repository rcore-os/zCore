pub(crate) mod ffi;

cfg_if! {
    if #[cfg(target_arch = "x86_64")] {
        #[path = "arch/x86_64/mod.rs"]
        mod arch;
        pub use self::arch::special as x86_64;
    } else if #[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))] {
        #[path = "arch/riscv/mod.rs"]
        mod arch;
        pub use self::arch::special as riscv;
    }
}

pub mod mem;
pub mod thread;
pub mod timer;

pub use self::arch::{config, context, cpu, interrupt, serial, vm};
pub use super::defs::{dev, rand, vdso};

hal_fn_impl_default!(rand, vdso, dev::fb, dev::input);

/// Initialize the HAL.
///
/// This function must be called at the beginning.
pub fn init(cfg: config::HalConfig) {
    unsafe { trapframe::init() };
    self::arch::init(cfg);
}
