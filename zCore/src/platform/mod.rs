cfg_if! {
    if #[cfg(feature = "libos")] {
        #[path = "libos/mod.rs"]
        mod arch;
    } else if #[cfg(target_arch = "x86_64")] {
        #[path = "x86/mod.rs"]
        mod arch;
    } else if #[cfg(target_arch = "riscv64")] {
        #[path = "riscv/mod.rs"]
        mod arch;
    }
}

pub use arch::consts;
