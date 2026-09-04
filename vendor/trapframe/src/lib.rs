//! Cross the user/kernel boundary through a function-like context switch.
//!
//! `trapframe` saves the current kernel context, enters a [`UserContext`], and
//! returns from `UserContext::run` when the user context traps. The trap is
//! therefore presented to kernel Rust code like the return value of an ordinary
//! function call: inspect the updated context, handle the event, then run it
//! again.
//!
//! The exact register layout and trap metadata are selected at compile time for
//! x86-64, AArch64, RISC-V 32/64, or little-endian MIPS.
//!
//! On hosted x86-64 and AArch64 targets, `UserContext::run_fncall` provides a
//! same-privilege variant for testing or embedding user code in a normal
//! process.
//!
//! # Typical flow
//!
//! 1. Call `init` once to install the architecture's trap entry points.
//! 2. Fill a [`UserContext`] with the initial instruction pointer, stack
//!    pointer, and arguments.
//! 3. Call `UserContext::run`.
//! 4. When it returns, inspect the saved registers and trap metadata.
//!
//! The low-level layouts are public because kernels commonly need direct access
//! to saved registers. They are `#[repr(C)]` and must stay synchronized with the
//! assembly routines in this crate.

#![no_std]
#![deny(warnings)]
#![deny(missing_docs)]
#![cfg_attr(target_arch = "mips", feature(asm_experimental_arch))]

#[cfg(target_arch = "x86_64")]
extern crate alloc;

#[cfg(target_arch = "x86_64")]
#[path = "arch/x86_64/mod.rs"]
mod arch;

#[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
#[path = "arch/riscv/mod.rs"]
mod arch;

#[cfg(target_arch = "mips")]
#[path = "arch/mipsel/mod.rs"]
pub mod arch;

#[cfg(target_arch = "aarch64")]
#[path = "arch/aarch64/mod.rs"]
pub mod arch;

pub use arch::*;
