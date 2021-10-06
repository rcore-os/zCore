use alloc::boxed::Box;
use core::time::Duration;

use naive_timer::Timer;
use spin::Mutex;

lazy_static! {
    static ref NAIVE_TIMER: Mutex<Timer> = Mutex::new(Timer::default());
}

hal_fn_impl! {
    impl mod crate::hal_fn::timer {
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
