//! Hardware Abstraction Layer

#![no_std]
#![feature(linkage)]
#![deny(warnings)]

extern crate alloc;

pub mod defs {
    use bitflags::bitflags;

    bitflags! {
        pub struct MMUFlags: usize {
            #[allow(clippy::identity_op)]
            const READ      = 1 << 0;
            const WRITE     = 1 << 1;
            const EXECUTE   = 1 << 2;
            const USER      = 1 << 3;
        }
    }

    pub type PhysAddr = usize;
    pub type VirtAddr = usize;
    pub const PAGE_SIZE: usize = 0x1000;
}

mod dummy;
mod future;
pub mod user;
pub mod vdso;

pub use self::defs::*;
pub use self::dummy::*;
pub use self::future::*;
pub use trapframe::{GeneralRegs, UserContext, VectorRegs};
