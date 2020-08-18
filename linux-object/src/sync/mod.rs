//! Useful synchronization primitives.
#![deny(missing_docs)]

pub use self::event_bus::*;
pub use self::semaphore::*;

mod event_bus;
mod semaphore;
