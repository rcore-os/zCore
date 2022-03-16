//! External interrupt request and handle.

cfg_if::cfg_if! {
    if #[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))] {
        mod riscv_intc;
        mod riscv_plic;

        /// Implementation of risc-v interrupt controller.
        #[doc(cfg(any(target_arch = "riscv32", target_arch = "riscv64")))]
        pub mod riscv {
            pub use super::riscv_intc::{Intc, ScauseIntCode};
            pub use super::riscv_plic::Plic;
        }
    } else if #[cfg(any(target_arch = "x86", target_arch = "x86_64"))] {
        mod x86_apic;

        /// Implementation of x86 Advanced Programmable Interrupt Controller.
        #[doc(cfg(any(target_arch = "x86", target_arch = "x86_64")))]
        pub mod x86 {
            pub use super::x86_apic::Apic;
        }
    }
}
