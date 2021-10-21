//! Hardware Abstraction Layer

#![cfg_attr(not(feature = "libos"), no_std)]
#![feature(asm)]
#![feature(doc_cfg)]
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

pub mod drivers;

cfg_if! {
    if #[cfg(feature = "libos")] {
        #[path = "libos/mod.rs"]
        mod imp;
    } else {
        #[path = "bare/mod.rs"]
        mod imp;
    }
}

pub(crate) use config::KCONFIG;
pub(crate) use kernel_handler::KHANDLER;

pub use common::{addr, console, defs::*, user};
pub use config::KernelConfig;
pub use imp::*;
pub use kernel_handler::KernelHandler;

#[cfg(any(feature = "smp", doc))]
#[doc(cfg(feature = "smp"))]
pub use imp::boot::{primary_init, primary_init_early, secondary_init};
