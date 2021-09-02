use alloc::boxed::Box;
use core::time::Duration;

/// Get current time.
pub fn timer_now() -> Duration {
    unimplemented!()
}

/// Set a new timer. After `deadline`, the `callback` will be called.
pub fn timer_set(_deadline: Duration, _callback: Box<dyn FnOnce(Duration) + Send + Sync>) {
    unimplemented!()
}

pub fn timer_set_next() {
    unimplemented!()
}

/// Check timers, call when timer interrupt happened.
pub fn timer_tick() {
    unimplemented!()
}
