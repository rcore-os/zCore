#[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
mod riscv;
#[cfg(target_arch = "x86_64")]
mod x86_64;

#[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
pub use self::riscv::*;
#[cfg(target_arch = "x86_64")]
pub use self::x86_64::*;
