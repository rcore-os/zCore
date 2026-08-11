//! Hardware Abstraction Layer

#![cfg_attr(not(feature = "libos"), no_std)]
#![cfg_attr(feature = "libos", feature(thread_id_value))]
#![feature(doc_cfg)]
// #![feature(core_intrinsics)]
#![allow(clippy::uninit_vec)]
#![allow(static_mut_refs)]
#![deny(warnings)]
// JUST FOR DEBUG
#![allow(dead_code)]

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
pub mod config;
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

pub use common::{addr, console, context, defs::*, ipi::*, user};
pub use config::KernelConfig;
pub use imp::{
    boot::{primary_init, primary_init_early, secondary_init},
    *,
};
pub use kernel_handler::KernelHandler;
pub use utils::{lazy_init::LazyInit, mpsc_queue::MpscQueue};
