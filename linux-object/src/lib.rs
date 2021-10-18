//! Linux kernel objects

#![no_std]
#![deny(warnings, unsafe_code, missing_docs)]
#![allow(clippy::upper_case_acronyms)]
#![feature(bool_to_option)]

#[macro_use]
extern crate alloc;

#[macro_use]
extern crate log;

// layer 0
pub mod error;

// layer 1
pub mod fs;

// layer 2
pub mod ipc;
pub mod loader;
pub mod process;
pub mod signal;
pub mod sync;
pub mod thread;
pub mod time;
