#![no_std]

cfg_if::cfg_if! {
    if #[cfg(all(target_os = "none", feature = "ticket"))] {
        extern crate alloc;
        mod interrupt;
        pub mod mcslock;
        pub mod rwlock;
        pub use {rwlock::*, mcslock::*};
        pub mod ticket;
        pub use ticket::{TicketMutex as Mutex, TicketMutexGuard as MutexGuard};
    } else if #[cfg(target_os = "none")] {
        extern crate alloc;
        mod interrupt;
        pub mod mcslock;
        pub mod rwlock;
        pub use {rwlock::*, mcslock::*};
        pub mod spin;
        pub use spin::{SpinMutex as Mutex, SpinMutexGuard as MutexGuard};
    } else {
        pub use spin::*;
    }
}
