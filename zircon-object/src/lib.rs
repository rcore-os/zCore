#![no_std]
#![deny(warnings, unsafe_code, unused_must_use)]
#![feature(asm, linkage)]
#![feature(drain_filter)]
#![feature(get_mut_unchecked)]

extern crate alloc;

#[macro_use]
extern crate log;

#[cfg(test)]
extern crate std;

mod error;
pub mod hal;
pub mod io;
pub mod ipc;
pub mod object;
pub mod resource;
pub mod task;
mod util;
pub mod vm;

pub use self::error::*;
