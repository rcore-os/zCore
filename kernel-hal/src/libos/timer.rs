//! Time and clock functions.

use async_std::task;
use nix::time::{clock_gettime, ClockId};
use std::time::Duration;

hal_fn_impl! {
    impl mod crate::hal_fn::timer {
        fn timer_now() -> Duration {
            let now = clock_gettime(ClockId::CLOCK_MONOTONIC).unwrap();
            Duration::new(now.tv_sec() as u64, now.tv_nsec() as u32)
        }

        fn timer_set(deadline: Duration, callback: Box<dyn FnOnce(Duration) + Send + Sync>) {
            task::spawn(async move {
                let now = timer_now();
                if deadline > now {
                    task::sleep(deadline - now).await;
                }
                callback(timer_now());
            });
        }
    }
}
