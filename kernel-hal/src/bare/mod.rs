mod ffi;

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

pub mod interrupt;
pub mod mem;
pub mod thread;
pub mod timer;
pub mod vm;

pub use super::defs::{dev, rand, vdso};

hal_fn_impl_default!(rand, vdso, dev::fb, dev::input);

pub use self::arch::{context, cpu, serial, HalConfig};

#[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
pub use self::arch::{BootInfo, GraphicInfo};

/// Initialize the HAL.
///
/// This function must be called at the beginning.
pub fn init(config: HalConfig) {
    unsafe { trapframe::init() };

    self::arch::init(config);
}
