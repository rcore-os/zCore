cfg_if::cfg_if! {
    if #[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))] {
        mod riscv_intc;
        mod riscv_plic;

        pub use riscv_intc::{RiscvIntc, RiscvScauseIntCode};
        pub use riscv_plic::RiscvPlic;
    }
}
