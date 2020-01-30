//! Hardware Abstraction Layer

#![no_std]
#![feature(linkage)]

extern crate alloc;

pub mod defs {
    use bitflags::bitflags;

    bitflags! {
        pub struct MMUFlags: usize {
            #[allow(clippy::identity_op)]
            const READ      = 1 << 0;
            const WRITE     = 1 << 1;
            const EXECUTE   = 1 << 2;
        }
    }

    pub type PhysAddr = usize;
    pub type VirtAddr = usize;
    pub const PAGE_SIZE: usize = 0x1000;
}

mod dummy;

pub use self::defs::*;
pub use self::dummy::*;
