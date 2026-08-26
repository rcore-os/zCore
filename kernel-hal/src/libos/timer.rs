//! Time and clock functions.

use async_std::task;
use std::time::{Duration, SystemTime};

hal_fn_impl! {
    impl mod crate::hal_fn::timer {
        fn timer_now() -> Duration {
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
        }

        fn timer_set(deadline: Duration, callback: Box<dyn FnOnce(Duration) + Send + Sync>) {
            task::spawn(async move {
                let dur = deadline - timer_now();
                task::sleep(dur).await;
                callback(timer_now());
            });
        }
    }
}

/// Wall-clock (libos: host time from `SystemTime`).
pub fn wall_clock_now() -> Duration {
    timer_now()
}

/// Adjust wall clock (libos: no effect on host; satisfies syscall for tests).
pub fn wall_clock_set(_target: Duration) {}

/// Wall-clock offset in nanoseconds (libos: the host clock is already
/// wall-clock, so there is nothing to add).
pub fn wall_clock_offset_ns() -> u64 {
    0
}

/// TSC multiplier for the vDSO (libos: there is no vDSO — processes are host
/// threads and read the host's own clock).
pub fn vdso_tsc_mult() -> Option<u64> {
    None
}

/// Register the clock-parameter observer (libos: nothing publishes a clock to
/// userspace, so the registration is inert).
pub fn set_clock_observer(_observer: fn()) {}

/// Force the TSC to be considered usable by userspace (libos: no vDSO).
pub fn set_force_tsc_invariant(_force: bool) {}

/// Bare-metal timer-callback containment flag (libos: always false).
pub fn in_timer_callback() -> bool {
    false
}
