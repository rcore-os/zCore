//! Useful synchronization primitives.
pub use self::event_bus::*;
pub use self::semaphore::*;

mod event_bus;
mod semaphore;
