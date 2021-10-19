pub mod interrupt;

pub use interrupt::*;

use kernel_hal::trap::consts::*;
// REF: https://github.com/rcore-os/trapframe-rs/blob/master/src/arch/x86_64/syscall.S
const SYSCALL: usize = 0x100;

