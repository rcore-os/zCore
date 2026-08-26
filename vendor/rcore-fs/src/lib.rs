#![cfg_attr(not(any(test, feature = "std")), no_std)]

extern crate alloc;

pub mod dev;
pub mod dirty;
pub mod file;
pub mod util;
pub mod vfs;

#[cfg(any(test, feature = "std"))]
mod std;
