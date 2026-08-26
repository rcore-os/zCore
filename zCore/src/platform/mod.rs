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
    } else if #[cfg(target_arch = "aarch64")] {
        #[path = "aarch64/mod.rs"]
        mod arch;
    }
}

// `arch::consts::*` puede quedar vacío según target/features; evitar warnings con `#![deny(warnings)]`.

// El allocator genérico (`memory.rs`, usado fuera de x86_64) necesita el offset
// físico→virtual de la plataforma. En riscv/aarch64 lo define `arch::consts`.
#[cfg(all(
    not(feature = "libos"),
    any(target_arch = "riscv64", target_arch = "aarch64")
))]
pub use arch::consts::phys_to_virt_offset;
