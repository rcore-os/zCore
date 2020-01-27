//! Linux syscall implementations

#![no_std]
#![deny(warnings, unsafe_code, unused_must_use, unreachable_patterns)]
#![feature(bool_to_option)]

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
mod loader;
mod process;

// layer 3
mod syscall;

pub use loader::LinuxElfLoader;
pub use process::ProcessExt;
pub use syscall::Syscall;
