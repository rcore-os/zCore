//! Zircon kernel objects

#![no_std]
#![deny(warnings, unsafe_code, unused_must_use)]
#![feature(asm)]
#![feature(linkage)]
#![feature(drain_filter)]
#![feature(get_mut_unchecked)]
#![feature(naked_functions)]
#![feature(ptr_offset_from)]
#![feature(range_is_empty)]
#![feature(new_uninit)]
#![feature(const_in_array_repeat_expressions)]

extern crate alloc;

#[macro_use]
extern crate log;

#[cfg(test)]
#[macro_use]
extern crate std;

pub mod debuglog;
pub mod dev;
mod error;
#[cfg(feature = "hypervisor")]
pub mod hypervisor;
pub mod ipc;
pub mod object;
pub mod signal;
pub mod task;
pub mod util;
pub mod vm;

pub use self::error::*;
