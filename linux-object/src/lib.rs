//! Linux kernel objects

#![no_std]
#![deny(warnings, unsafe_code, unused_must_use, unreachable_patterns)]
#![feature(bool_to_option)]

extern crate alloc;

#[macro_use]
extern crate log;

#[macro_use]
extern crate num_derive;

// layer 0
pub mod error;

// layer 1
pub mod fs;

// layer 2
pub mod loader;
pub mod process;
pub mod signal;
pub mod sync;
pub mod thread;
