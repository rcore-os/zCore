#![no_std]
#![deny(unsafe_code, unused_must_use)]

#[macro_use]
extern crate alloc;

#[macro_use]
extern crate bitflags;

#[macro_use]
extern crate log;

#[cfg(test)]
extern crate std;

mod error;
pub mod io;
pub mod ipc;
pub mod object;
pub mod syscall;
pub mod task;
mod util;
pub mod vm;

pub use self::error::*;
