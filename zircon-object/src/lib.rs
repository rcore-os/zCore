#![no_std]
#![deny(unsafe_code, unused_must_use)]
#![feature(asm, linkage)]
#![feature(drain_filter)]

#[macro_use]
extern crate alloc;

#[macro_use]
extern crate bitflags;

#[macro_use]
extern crate log;

#[cfg(test)]
extern crate std;

mod error;
mod hal;
pub mod io;
pub mod ipc;
pub mod object;
pub mod task;
mod util;
pub mod vm;

pub use self::error::*;
pub use self::hal::serial_write;
