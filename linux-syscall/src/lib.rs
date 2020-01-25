//! Linux syscall implementations

#![no_std]
#![deny(unsafe_code, unused_must_use, unreachable_patterns)]

#[macro_use]
extern crate alloc;

#[macro_use]
extern crate log;

// layer 0
mod error;
mod util;

// layer 1
mod fs;

// layer 2
mod process;

// layer 3
mod syscall;

pub use process::ProcessExt;
pub use syscall::Syscall;
