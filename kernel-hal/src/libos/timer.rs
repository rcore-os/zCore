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
            let dur = deadline - timer_now();
            task::spawn(async move {
                task::sleep(dur).await;
                callback(timer_now());
            });
        }
    }
}
