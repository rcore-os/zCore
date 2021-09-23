//! Hardware Abstraction Layer

#![cfg_attr(not(feature = "libos"), no_std)]
#![feature(asm)]
#![deny(warnings)]

extern crate alloc;
#[macro_use]
extern crate log;
#[macro_use]
extern crate cfg_if;
#[macro_use]
extern crate lazy_static;

#[macro_use]
mod macros;

mod common;
mod config;
mod hal_fn;
mod kernel_handler;
mod utils;

cfg_if! {
    if #[cfg(feature = "libos")] {
        #[path = "libos/mod.rs"]
        mod imp;
    } else {
        #[path = "bare/mod.rs"]
        mod imp;
    }
}

pub use common::{addr, defs::*, drivers, serial, user};
pub use config::*;
pub use imp::*;
pub use kernel_handler::*;
