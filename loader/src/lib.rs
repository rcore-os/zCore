//! Linux and Zircon user programs loader and runner.

#![no_std]
#![feature(asm)]
#![feature(doc_cfg)]
#![deny(warnings, unused_must_use, missing_docs)]

extern crate alloc;
#[macro_use]
extern crate log;
#[macro_use]
extern crate cfg_if;

cfg_if! {
    if #[cfg(any(feature = "linux", doc))] {
        #[doc(cfg(feature = "linux"))]
        pub mod linux;
    }
}

cfg_if! {
    if #[cfg(any(feature = "zircon", doc))] {
        #[doc(cfg(feature = "zircon"))]
        pub mod zircon;
    }
}
