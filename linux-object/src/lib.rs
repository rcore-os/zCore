//! Linux kernel objects

#![no_std]
#![deny(warnings)]
// #![deny(missing_docs)] 形同虚设了
#![allow(clippy::upper_case_acronyms)]
#![allow(clippy::uninit_vec)]

#[cfg(test)]
#[macro_use]
extern crate std;

#[macro_use]
extern crate alloc;

#[macro_use]
extern crate log;

// layer 0
pub mod boot_trace;
pub mod error;

// layer 1
pub mod fs;

// layer 2
pub mod ipc;
pub mod loadavg;
pub mod loader;
pub mod net;
pub mod perf;
pub mod process;
pub mod signal;
pub mod sync;
pub mod thread;
pub mod time;
pub mod uname;
pub mod vdso;
