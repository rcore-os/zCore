//! Hardware Abstraction Layer

#![no_std]
#![feature(linkage)]
#![deny(warnings)]

extern crate alloc;

pub mod defs {
    #[cfg(target_arch = "x86_64")]
    include!("arch/x86_64.rs");
    #[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
    include!("arch/riscv.rs");

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
