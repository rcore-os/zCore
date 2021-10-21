cfg_if! {
    if #[cfg(target_arch = "x86_64")] {
        #[path = "arch/x86_64/mod.rs"]
        mod arch;
        pub use self::arch::special as x86_64;
    } else if #[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))] {
        #[path = "arch/riscv/mod.rs"]
        mod arch;
    }
}

pub mod boot;
pub mod mem;
pub mod thread;
pub mod timer;

pub use self::arch::{config, context, cpu, interrupt, vm};
pub use super::hal_fn::{rand, vdso};

hal_fn_impl_default!(rand, vdso);

/// Non-SMP initialization.
#[cfg(any(not(feature = "smp"), doc))]
#[doc(cfg(not(feature = "smp")))]
pub fn init(cfg: crate::KernelConfig, handler: &'static impl crate::KernelHandler) {
    boot::primary_init_early(cfg, handler);
    boot::primary_init();
}
