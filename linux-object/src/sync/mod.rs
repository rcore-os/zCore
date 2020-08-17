//! Useful synchronization primitives.
pub use self::event_bus::*;
pub use self::semaphore::*;

mod semaphore;
mod event_bus;
