cfg_if! {
    if #[cfg(target_arch = "x86_64")] {
        #[path = "arch/x86_64/mod.rs"]
        mod arch;
        pub use self::arch::{special as x86_64, timer_interrupt_vector};
    } else if #[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))] {
        #[path = "arch/riscv/mod.rs"]
        pub mod arch;
        pub use self::arch::{sbi, timer_interrupt_vector};
    } else if #[cfg(target_arch = "aarch64")] {
        #[path = "arch/aarch64/mod.rs"]
        pub mod arch;
        pub use self::arch::timer_interrupt_vector;
    }
}

pub mod boot;
pub mod mem;
pub mod net;
pub mod thread;
pub mod timer;

pub use self::arch::{config, cpu, interrupt, vm};
pub use super::hal_fn::{rand, vdso};

hal_fn_impl_default!(rand, vdso);
