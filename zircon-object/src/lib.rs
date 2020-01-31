//! Zircon kernel objects

#![no_std]
#![deny(warnings, unsafe_code, unused_must_use)]
#![feature(asm)]
#![feature(drain_filter)]
#![feature(get_mut_unchecked)]
#![feature(naked_functions)]

extern crate alloc;

#[macro_use]
extern crate log;

#[cfg(test)]
#[macro_use]
extern crate std;

mod error;
pub mod ipc;
pub mod object;
pub mod resource;
pub mod signal;
pub mod task;
mod util;
pub mod vm;

pub use self::error::*;
