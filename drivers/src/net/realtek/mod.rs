#![allow(unused)]
#![allow(non_camel_case_types)]

use super::Provider;
use super::{phys_to_virt, virt_to_phys};

#[macro_use]
mod log {
    macro_rules! trace {
        ($($arg:expr),*) => { $( let _ = $arg; )* };
    }
    macro_rules! debug {
        ($($arg:expr),*) => { $( let _ = $arg; )* };
    }
    macro_rules! info {
        ($($arg:expr),*) => { $( let _ = $arg; )*};
    }
    macro_rules! warn {
        ($($arg:expr),*) => { $( let _ = $arg; )*};
    }
    macro_rules! error {
        ($($arg:expr),*) => { $( let _ = $arg; )* };
    }
}

pub mod mii;
pub mod rtl8211f;
mod utils;
