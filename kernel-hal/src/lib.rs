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
mod macros;

mod common;
mod defs;

pub use common::{addr, defs::*, future, user};

cfg_if! {
    if #[cfg(feature = "libos")] {
        mod libos;
        pub use self::libos::*;
    } else {
        mod bare;
        pub use self::bare::*;
    }
}
