//! Hardware Abstraction Layer

#![cfg_attr(not(feature = "libos"), no_std)]
#![cfg_attr(feature = "libos", feature(thread_id_value))]
#![feature(asm)]
#![feature(doc_cfg)]
#![allow(clippy::uninit_vec)]
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

pub use common::{addr, console, context, defs::*, user};
pub use config::KernelConfig;
pub use imp::*;
pub use kernel_handler::KernelHandler;

#[cfg(any(feature = "smp", doc))]
#[doc(cfg(feature = "smp"))]
pub use imp::boot::{primary_init, primary_init_early, secondary_init};

mod interrupt_ffi {
    #[no_mangle]
    extern "C" fn intr_on() {
        super::interrupt::intr_on();
    }

    #[no_mangle]
    extern "C" fn intr_off() {
        super::interrupt::intr_off();
    }

    #[no_mangle]
    extern "C" fn intr_get() -> bool {
        super::interrupt::intr_get()
    }

    #[no_mangle]
    extern "C" fn cpu_id() -> u8 {
        super::cpu::cpu_id()
    }
}