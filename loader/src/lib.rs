//! Linux and Zircon user programs loader and runner.

#![no_std]
#![deny(warnings, missing_docs)]
#![feature(doc_cfg)]

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
