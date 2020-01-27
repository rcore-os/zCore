#[cfg(target_arch = "x86_64")]
include!("x86_64.rs");

#[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
include!("riscv.rs");
