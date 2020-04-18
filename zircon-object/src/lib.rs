//! Zircon kernel objects

#![no_std]
#![deny(warnings, unsafe_code, unused_must_use)]
#![feature(asm)]
#![feature(linkage)]
#![feature(drain_filter)]
#![feature(get_mut_unchecked)]
#![feature(naked_functions)]
#![feature(ptr_offset_from)]

extern crate alloc;

#[macro_use]
extern crate log;

#[cfg(test)]
#[macro_use]
extern crate std;

pub mod debuglog;
mod error;
pub mod exception;
pub mod ipc;
pub mod object;
pub mod resource;
pub mod signal;
pub mod task;
pub mod util;
pub mod vm;

pub use self::error::*;
