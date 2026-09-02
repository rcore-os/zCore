//! Interrupt-safe synchronization primitives used by the kernel and drivers.

cfg_if::cfg_if! {
    if #[cfg(target_os = "none")] {
        mod interrupt;
        mod rwlock;
        mod ticket;

        pub use rwlock::{RwLock, RwLockReadGuard, RwLockUpgradableGuard, RwLockWriteGuard};
        pub use ticket::{TicketMutex as Mutex, TicketMutexGuard as MutexGuard};
    } else {
        pub use spin::{Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockUpgradableGuard, RwLockWriteGuard};
    }
}
