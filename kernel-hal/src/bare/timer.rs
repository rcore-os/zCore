//! Time and clock functions.

use alloc::boxed::Box;
use core::time::Duration;

use core::sync::atomic::{AtomicBool, Ordering};
use lock::Mutex;
use naive_timer::Timer;

#[allow(dead_code)]
pub(super) const TICKS_PER_SEC: u64 = 100;

lazy_static! {
    static ref NAIVE_TIMER: Mutex<Timer> = Mutex::new(Timer::default());
    static ref FIRST: AtomicBool = AtomicBool::new(false);
}

hal_fn_impl! {
    impl mod crate::hal_fn::timer {
        fn timer_set_first() {
            if !FIRST.load(Ordering::Relaxed) {
                FIRST.store(true, Ordering::Relaxed);
                super::arch::timer_init();
            }
        }

        fn timer_now() -> Duration {
            super::arch::timer::timer_now()
        }

        fn timer_set(deadline: Duration, callback: Box<dyn FnOnce(Duration) + Send + Sync>) {
            NAIVE_TIMER.lock().add(deadline, callback);
        }

        fn timer_tick() {
            NAIVE_TIMER.lock().expire(timer_now());
        }
    }
}
